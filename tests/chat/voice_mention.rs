//! R06/R07 characterization (no behaviour change): what the chat pilot
//! sends the model today. Persona/profile placement in the one system
//! message, the scheduler voice note sent last on every round, alias and
//! effort, the routed tool list, and how mentions of the bot and of other
//! members appear in the source text. Wholly invented fixtures on the loop
//! vector's synthetic world; the fake provider records every request.

use std::sync::Arc;

use chrono::Utc;
use kanade::chat::answer::{AnswerDeps, Generation, Question, answer};
use kanade::chat::context::{
    Conversations, Parent, QuestionMessage, Reference, assemble, build_turns, system_prompt,
};
use kanade::chat::persona::{
    CompiledPersona, PersonaId, PersonaRoot, ProfileId, VoiceSource, parse_profile,
};
use kanade::chat::pilot::ChatPilot;
use kanade::chat::prompts::{DEFAULT_VOICE, REMINDER_PREFIX};
use kanade::chat::sanitize::{ScheduleDefaults, schedule_defaults};
use kanade::chat::tools::ToolContext;
use kanade::domain::scheduler::Clock;
use kanade::extract::prompt::estimate_tokens;
use kanade::infrastructure::llm::governor::{DEFAULT_TOOL_ROUNDS, Governor, Role};
use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, Effort, FakeAction, FakeProvider, FinishReason, Message,
    ToolCall, wire_body,
};
use serde_json::{Value, json};

use crate::looping::{Ports, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::wire::kanade;
use crate::world::World;

const BOT: &str = "5000";
const ROLE: &str = "5001";
const ASKER: &str = "11";
const CHANNEL: &str = "700";
const MODEL: &str = "synthetic-chat";
/// The driver's monotonic clock is irrelevant here; any fixed value works.
const NOW: f64 = 1000.0;

/// An invented reply profile with its own voice cue.
const PROFILE: &str = "schema_version: 1\nid: brisk\nlabel: Brisk\nvoice: Short, dry and a little smug.\nprompt: |\n  # Reply profile\n\n  Keep replies brisk; one tease at most.\n";

fn wants(calls: &[(&str, &str, Value)]) -> FakeAction {
    FakeAction::Response(CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: MODEL.into(),
        content: None,
        tool_calls: calls
            .iter()
            .map(|(id, name, arguments)| ToolCall {
                id: (*id).into(),
                name: (*name).into(),
                arguments: arguments.to_string(),
            })
            .collect(),
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })
}

fn words(text: &str) -> FakeAction {
    FakeAction::Response(said(MODEL, text))
}

fn input() -> Value {
    load("loop.json")["cases"][0]["input"].clone()
}

fn bundle_with(profile: Option<&str>) -> CompiledPersona {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas");
    let root = PersonaRoot::open(&root).expect("tracked personas");
    let bundle = root
        .load_bundle(&PersonaId::parse("kanade").expect("id"))
        .expect("kanade bundle")
        .value;
    let profile = profile.map(|text| {
        parse_profile(text, &ProfileId::parse("brisk").expect("id")).expect("invented profile")
    });
    CompiledPersona::compile(&bundle, profile.as_ref())
}

fn asked(content: &str, reference: Option<Reference>) -> QuestionMessage {
    QuestionMessage {
        id: "9100".into(),
        author_id: ASKER.into(),
        content: content.into(),
        reference,
    }
}

fn replying_to(id: &str, author: &str, content: &str) -> Option<Reference> {
    Some(Reference {
        message_id: Some(id.into()),
        resolved: Some(Box::new(Parent {
            id: id.into(),
            author_id: Some(author.into()),
            content: Some(content.into()),
            reference: None,
        })),
    })
}

/// The system prompt the driver builds for this world's clock.
fn system(world: &World, persona: &CompiledPersona) -> String {
    system_prompt(
        persona,
        world.clock.now().with_timezone(&Utc),
        world.zone,
        (world.policy.reset_weekday, world.policy.reset_time),
        MODEL,
        "",
    )
}

/// The driver's conversation (`driver/run.rs`): turns, system prompt, assemble.
fn conversation(
    world: &World,
    persona: &CompiledPersona,
    message: &QuestionMessage,
) -> Vec<Message> {
    let mut state = Conversations::new(2700.0);
    let turns = build_turns(&mut state, message, CHANNEL, NOW, BOT, &world.guild);
    assemble(&turns, system(world, persona), 16_384, 1024)
}

/// The driver's trusted tool context (`driver/run.rs` `tool_context`).
fn tool_context(world: &World, message: &QuestionMessage) -> ToolContext {
    let mut ctx = world.context(&json!({"author_id": ASKER, "channel_id": CHANNEL}));
    let defaults = schedule_defaults(&message.content, Some(BOT), Some(ROLE));
    ctx.force_all_channels = defaults.force_all_channels;
    ctx.force_channel_scope = defaults.force_channel_scope;
    ctx.force_group_schedule = defaults.force_group_schedule;
    ctx.self_schedule_requested = defaults.self_schedule_requested;
    ctx.upcoming_only = defaults.upcoming_only;
    ctx.next_only = defaults.next_only;
    ctx
}

struct Captured {
    requests: Vec<ChatRequest>,
    bodies: Vec<Value>,
    generation: Generation,
    system: String,
    reminder: String,
}

/// One question through `answer`, as the driver assembles it; `route_effort`
/// is the live chat route's effort (`None` keeps the caller's).
async fn capture(
    persona: &CompiledPersona,
    message: &QuestionMessage,
    actions: Vec<FakeAction>,
    route_effort: Option<Effort>,
) -> Captured {
    let input = input();
    let mut world = World::new(&input).await;
    let caps = capabilities(&input["caps"]);
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: caps.clone(),
    });
    let (governor, client): (Arc<Governor>, _) = client(Some(MODEL), provider.clone());
    if route_effort.is_some() {
        assert!(governor.set_effort(Role::Chat, route_effort));
    }
    let ctx = tool_context(&world, message);
    let conversation = conversation(&world, persona, message);
    let system = match &conversation[0] {
        Message::System { content } => content.clone(),
        other => panic!("first message {other:?}"),
    };
    let reminder = persona.voice_reminder();
    let ports = Ports::default();
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation,
            reminder: reminder.clone(),
            offer: ChatPilot::route(&message.content, None, ctx.read_only),
            settings: settings(&input, DEFAULT_TOOL_ROUNDS),
            profanity: None,
        };
        let deps = AnswerDeps {
            client: &client,
            route: None,
        };
        answer(&deps, question, &guild, &mut proposer, &ports).await
    };
    let requests = provider.fake.requests();
    let bodies = requests
        .iter()
        .map(|request| wire_body(request, &caps).expect("sent bodies shape"))
        .collect();
    Captured {
        requests,
        bodies,
        generation,
        system,
        reminder,
    }
}

fn roles(body: &Value) -> Vec<&str> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["role"].as_str().expect("role"))
        .collect()
}

fn tool_names(request: &ChatRequest) -> Vec<&str> {
    request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect()
}

fn user_texts(request: &ChatRequest) -> Vec<&str> {
    request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::User { content } => Some(content.as_str()),
            _ => None,
        })
        .collect()
}

/// Byte offset of `needle` in `haystack`, which must contain it.
fn at(haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("system prompt lacks {needle:?}"))
}

/// Persona and profile text sit at the top of the single system message,
/// ahead of the code-owned policies, the clock/runtime facts and the voice
/// cue; no profile text appears anywhere else.
#[tokio::test]
async fn the_system_prompt_layers_persona_profile_policies_then_trusted_facts() {
    let world = World::new(&input()).await;
    let plain = bundle_with(None);
    let styled = bundle_with(Some(PROFILE));
    assert_eq!(plain.provenance().voice, VoiceSource::Default);
    assert_eq!(styled.provenance().voice, VoiceSource::Profile);
    assert_eq!(plain.effective_voice(), DEFAULT_VOICE);

    let system = system(&world, &styled);
    let order = [
        at(&system, "# Persona: OtonoseKanade"),
        at(&system, "Give every reply a playful kusogaki"),
        at(&system, "# Reply profile"),
        at(&system, "# Assistant scope"),
        at(&system, "# Scheduler policy"),
        at(&system, "# Grounding, privacy and presentation policy"),
        at(&system, "# Boss knowledge policy"),
        at(&system, "Right now it is "),
        at(
            &system,
            "You are a Discord bot for this guild's boss schedule.",
        ),
        at(
            &system,
            "Before you answer, remember your voice: Short, dry and a little smug.",
        ),
    ];
    assert_eq!(order[0], 0, "the persona identity opens the prompt");
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");

    // Persona text (identity + behaviour + profile) versus everything after it.
    let persona_end = order[3];
    let persona_tokens = estimate_tokens(&system[..persona_end]);
    let rest_tokens = estimate_tokens(&system[persona_end..]);
    eprintln!(
        "system prompt: {} bytes, ~{} tokens; persona ~{persona_tokens}, policies+facts ~{rest_tokens}",
        system.len(),
        estimate_tokens(&system)
    );
    assert!(
        persona_tokens > rest_tokens,
        "persona outweighs policies today"
    );

    // The plain bundle has no profile section and the code-owned voice cue.
    let plain_system = self::system(&world, &plain);
    assert!(!plain_system.contains("# Reply profile"));
    assert!(plain_system.ends_with(
        "Before you answer, remember your voice: Answer in the voice defined above. The schedule facts must be exact; everything around them is said in character.\nThis voice changes presentation only; trusted facts, operating policy, privacy and tool authority still control the answer."
    ));
    // Nothing tells the model which Discord mention is its own: the persona
    // names "OtonoseKanade", never `<@5000>` or the managed role.
    for prompt in [&system, &plain_system] {
        assert!(!prompt.contains(BOT) && !prompt.contains(ROLE), "no own id");
        assert!(!prompt.contains("@Kanade"));
    }
}

/// Every round resends the same system message first and the scheduler
/// voice note (a `user` message) last; the question and the tool exchange
/// sit between them. Roles are `system`/`user`/`assistant`/`tool`, never
/// `developer`. The live route's effort wins over the caller's.
#[tokio::test]
async fn every_round_resends_the_system_prompt_and_ends_with_the_voice_note() {
    let persona = bundle_with(Some(PROFILE));
    let message = asked("<@5000> what are my runs this week?", None);
    let run = capture(
        &persona,
        &message,
        vec![
            wants(&[(
                "c1",
                "get_schedule",
                json!({"participant": "<@5000>", "week": "this"}),
            )]),
            words("Eh? Here, omaera."),
        ],
        Some(Effort::High),
    )
    .await;
    assert_eq!(run.requests.len(), 2);
    assert!(
        run.generation.failure.is_none(),
        "{:?}",
        run.generation.failure
    );
    assert!(
        run.reminder.starts_with(REMINDER_PREFIX)
            && run
                .reminder
                .contains("Write your reply in your own voice: Short, dry and a little smug.")
    );

    assert_eq!(roles(&run.bodies[0]), ["system", "user", "user"]);
    assert_eq!(
        roles(&run.bodies[1]),
        ["system", "user", "assistant", "tool", "user"]
    );
    for (request, body) in run.requests.iter().zip(&run.bodies) {
        assert_eq!(
            request.messages[0],
            Message::System {
                content: run.system.clone()
            }
        );
        assert_eq!(
            request.messages.last(),
            Some(&Message::User {
                content: run.reminder.clone()
            })
        );
        assert_eq!(request.model, MODEL);
        assert_eq!(request.reasoning, Some(Effort::High), "route effort wins");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["temperature"], 0.7);
        assert_eq!(body["max_tokens"], 1024);
    }
    // The question reaches the model with the raw bot mention.
    assert_eq!(
        user_texts(&run.requests[0])[0],
        "Alvin tan: <@5000> what are my runs this week?"
    );
    // A plain read question: the read bundle plus `request_tools`.
    assert_eq!(
        tool_names(&run.requests[0]),
        [
            "get_schedule",
            "get_run",
            "list_bosses",
            "get_pending",
            "list_fixed",
            "request_tools"
        ]
    );
    // The model copied the bot mention as `participant`: read as the asker.
    let tool = run.requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            Message::Tool { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .expect("tool result");
    assert!(tool.starts_with("**Your "), "{tool}");
    let tool_tokens = estimate_tokens(tool);
    let system_tokens = estimate_tokens(&run.system);
    eprintln!(
        "round 2: system ~{system_tokens} tokens, tool result ~{tool_tokens}, reminder ~{}, tools offered {}",
        estimate_tokens(&run.reminder),
        run.requests[1].tools.len()
    );
}

/// Without a live route effort the caller's (vector `low`) is sent.
#[tokio::test]
async fn the_caller_effort_applies_when_the_route_has_none() {
    let persona = kanade();
    let message = asked("<@5000> hi", None);
    let run = capture(&persona, &message, vec![words("Hmph. Hi.")], None).await;
    assert_eq!(run.requests.len(), 1);
    assert_eq!(run.requests[0].reasoning, Some(Effort::Low));
    assert_eq!(run.bodies[0]["reasoning_effort"], "low");
}

/// Write words route the run-change bundle up front: more tool schema
/// text rides along with the persona on the first round.
#[tokio::test]
async fn write_words_add_the_run_change_bundle_to_the_first_round() {
    let persona = kanade();
    let message = asked("<@5000> move hard lotus to 9pm", None);
    let run = capture(&persona, &message, vec![words("Which day, omaera?")], None).await;
    assert_eq!(
        tool_names(&run.requests[0]),
        [
            "get_schedule",
            "get_run",
            "list_bosses",
            "get_pending",
            "list_fixed",
            "request_tools",
            "propose_move",
            "propose_add",
            "propose_cancel",
            "propose_rsvp"
        ]
    );
}

/// What the model sees for each mention shape, and the trusted
/// self-only reading of the same source text. Mentions are passed through
/// verbatim (frozen `context.json` pins `Alvin tan: <@5000> and what about
/// …`); only `schedule_defaults` removes the bot's own user/role mention.
#[tokio::test]
async fn mentions_reach_the_model_verbatim_and_only_self_only_text_is_trusted() {
    let world = World::new(&input()).await;
    let persona = kanade();
    let self_only = ScheduleDefaults {
        self_schedule_requested: true,
        ..ScheduleDefaults::default()
    };
    let not_self = ScheduleDefaults::default();
    let cases: [(&str, &str, ScheduleDefaults); 9] = [
        // Direct mention, nickname form and the managed role.
        (
            "<@5000> what are my runs this week?",
            "Alvin tan: <@5000> what are my runs this week?",
            self_only,
        ),
        (
            "<@!5000> what are my runs this week?",
            "Alvin tan: <@!5000> what are my runs this week?",
            self_only,
        ),
        (
            "<@&5001> what are my runs this week?",
            "Alvin tan: <@&5001> what are my runs this week?",
            self_only,
        ),
        // Trailing mention.
        (
            "what are my runs this week <@5000>",
            "Alvin tan: what are my runs this week <@5000>",
            self_only,
        ),
        // Mixed: self plus another member never becomes self-only.
        (
            "<@5000> what are my runs and <@22>'s runs this week?",
            "Alvin tan: <@5000> what are my runs and <@22>'s runs this week?",
            not_self,
        ),
        // Third party only.
        (
            "<@5000> what are <@22>'s runs this week?",
            "Alvin tan: <@5000> what are <@22>'s runs this week?",
            not_self,
        ),
        // An unknown extra mention suppresses the fallback.
        (
            "<@5000> what are my runs this week <@777>",
            "Alvin tan: <@5000> what are my runs this week <@777>",
            not_self,
        ),
        // Addressing by name or with a greeting is not stripped: no fallback.
        (
            "Kanade, what are my runs this week?",
            "Alvin tan: Kanade, what are my runs this week?",
            not_self,
        ),
        (
            "hey <@5000> what are my runs this week?",
            "Alvin tan: hey <@5000> what are my runs this week?",
            not_self,
        ),
    ];
    for (source, seen, defaults) in cases {
        let messages = conversation(&world, &persona, &asked(source, None));
        assert_eq!(messages.len(), 2, "{source}");
        assert_eq!(
            messages[1],
            Message::User {
                content: seen.into()
            },
            "{source}"
        );
        let got = schedule_defaults(source, Some(BOT), Some(ROLE));
        assert_eq!(
            got.self_schedule_requested, defaults.self_schedule_requested,
            "{source}: {got:?}"
        );
    }
}

/// Replies: the bot's own parent is an assistant turn (no mention needed to
/// summon); a member's parent keeps its own mention text.
#[tokio::test]
async fn reply_parents_keep_their_text_and_the_bots_answer_is_an_assistant_turn() {
    let world = World::new(&input()).await;
    let persona = kanade();
    let to_bot = asked(
        "and tomorrow?",
        replying_to(
            "8400",
            BOT,
            "Your 1 upcoming run this boss week · All channels",
        ),
    );
    assert_eq!(
        conversation(&world, &persona, &to_bot)[1..],
        [
            Message::Assistant {
                content: Some("Your 1 upcoming run this boss week · All channels".into()),
                tool_calls: Vec::new(),
            },
            Message::User {
                content: "Alvin tan: and tomorrow?".into()
            },
        ]
    );
    let to_member = asked(
        "same question <@5000>",
        replying_to("8401", "33", "<@5000> is hard lotus still on? ask <@44>"),
    );
    assert_eq!(
        conversation(&world, &persona, &to_member)[1..],
        [
            Message::User {
                content: "Priya: <@5000> is hard lotus still on? ask <@44>".into()
            },
            Message::User {
                content: "Alvin tan: same question <@5000>".into()
            },
        ]
    );
}

/// The reserved clean retry keeps only the system prompt, the asker's
/// message and the voice note: no history, tools or tool results.
#[tokio::test]
async fn the_clean_retry_keeps_the_persona_question_and_voice_note_only() {
    let persona = bundle_with(Some(PROFILE));
    let message = asked(
        "and tomorrow?",
        replying_to(
            "8400",
            BOT,
            "Your 1 upcoming run this boss week · All channels",
        ),
    );
    let run = capture(
        &persona,
        &message,
        vec![FakeAction::Malformed, words("Tomorrow? Nothing, omaera.")],
        None,
    )
    .await;
    assert_eq!(run.requests.len(), 2);
    assert!(run.generation.clean_retry);
    assert_eq!(
        roles(&run.bodies[0]),
        ["system", "assistant", "user", "user"]
    );
    assert_eq!(roles(&run.bodies[1]), ["system", "user", "user"]);
    assert!(run.requests[1].tools.is_empty());
    assert_eq!(
        user_texts(&run.requests[1]),
        ["Alvin tan: and tomorrow?", run.reminder.as_str()]
    );
}
