//! The v5 question loop beyond v4's behaviour: dynamic tool rounds and
//! `request_tools`, card delivery failure, the clean retry on a content
//! filter, identity encoding across rounds, and the chat-log row.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kanade::chat::answer::{
    AnswerDeps, AnswerFailure, CARD_NOT_POSTED, Generation, Question, answer, chat_outcome,
    interaction,
};
use kanade::chat::sanitize::shape_reply;
use kanade::chat::tools::bundles::{Bundle, NO_ROUND_LEFT, ToolOffer};
use kanade::chat::tools::propose::Proposer;
use kanade::chat::tools::{REFUSED, ToolContext, UNKNOWN};
use kanade::domain::drafts::{DraftStatus, ProposalSource, ProposalStore, SUPERSEDED};
use kanade::domain::model_log::{ChatInteraction, ChatOutcome, ModelLogStore};
use kanade::domain::proposals::{ChangeKind, ProposedChange};
use kanade::domain::scheduler::Clock;
use kanade::domain::scheduler::{ProposalRequest, SchedulerService, Supersede};
use kanade::infrastructure::llm::governor::{DEFAULT_TOOL_ROUNDS, Governor, Role};
use kanade::infrastructure::llm::{ChatRequest, Effort, FakeAction, FakeProvider, Message};
use kanade::infrastructure::store::SqliteStore;
use serde_json::{Value, json};

use crate::looping::{Ports, V4_TOOL_ROUNDS, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::slow_store::SlowStore;
use crate::support::world_input;
use crate::wire::kanade;
use crate::world::WorldOn;

type World = WorldOn<SqliteStore>;

const MODEL: &str = "synthetic-chat";

fn wants(calls: &[(&str, &str, Value)]) -> FakeAction {
    let calls = calls
        .iter()
        .map(|(id, name, arguments)| (*id, *name, arguments));
    crate::model::wants(MODEL, calls)
}

/// `propose_move` of hstar to Thu 22:00 (call `m1`).
fn move_hstar() -> FakeAction {
    let to = json!({"run_query": "hstar", "to_when": "thu 22:00"});
    wants(&[("m1", "propose_move", to)])
}

fn words(text: &str) -> FakeAction {
    FakeAction::Response(said(MODEL, text))
}

fn filtered() -> FakeAction {
    crate::model::filtered(MODEL)
}

/// The system prompt and Alvin's `line`.
fn conversation(line: &str) -> Vec<Message> {
    vec![
        Message::System {
            content: "SYSTEM".into(),
        },
        Message::User {
            content: format!("Alvin tan: {line}"),
        },
    ]
}

/// Routes chat to an external provider.
fn external(governor: &Governor) {
    governor.set_external(Role::Chat, true);
}

struct Run {
    generation: Generation,
    requests: Vec<ChatRequest>,
    /// The `x-request-id`s the provider was handed, in order.
    request_ids: Vec<String>,
    world: World,
    input: Value,
    ctx: ToolContext,
    question: String,
    _governor: Arc<Governor>,
}

impl Run {
    /// The chat-log row of this question at the world's clock.
    fn row(&self, id: &str, reasoning: Option<Effort>, latency_ms: u64) -> ChatInteraction {
        interaction(
            id.into(),
            self.world.clock.now().with_timezone(&Utc),
            &self.ctx,
            &self.question,
            &self.generation,
            MODEL,
            reasoning,
            latency_ms,
        )
    }
}

async fn run(
    actions: Vec<FakeAction>,
    offer: ToolOffer,
    tool_rounds: u8,
    question: &str,
    ports: &impl kanade::chat::answer::ChatPorts,
) -> Run {
    run_routed(actions, offer, tool_rounds, question, ports, |_| {}).await
}

/// `run` with eight tool rounds and recording ports.
async fn ask(actions: Vec<FakeAction>, offer: ToolOffer, question: &str) -> Run {
    run(actions, offer, 8, question, &Ports::default()).await
}

/// `route` adjusts the governor before the question.
async fn run_routed(
    actions: Vec<FakeAction>,
    offer: ToolOffer,
    tool_rounds: u8,
    question: &str,
    ports: &impl kanade::chat::answer::ChatPorts,
    route: impl FnOnce(&Governor),
) -> Run {
    let input = world_input();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (governor, client) = client(Some(MODEL), provider.clone());
    route(&governor);
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let persona = kanade();
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation: conversation(question),
            profanity: None,
            reminder: persona.voice_reminder(),
            offer,
            settings: settings(&input, tool_rounds),
        };
        answer(&deps, question, &guild, &mut proposer, ports).await
    };
    Run {
        generation,
        question: question.to_owned(),
        requests: provider.fake.requests(),
        request_ids: provider.fake.request_ids(),
        world,
        input,
        ctx,
        _governor: governor,
    }
}

fn tool_names(request: &ChatRequest) -> Vec<&str> {
    request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect()
}

#[test]
fn the_default_round_cap_is_the_user_decision() {
    assert_eq!(DEFAULT_TOOL_ROUNDS, 8);
    assert_eq!(
        u32::from(DEFAULT_TOOL_ROUNDS),
        kanade::chat::tools::bundles::DEFAULT_TOOL_ROUNDS
    );
}

#[tokio::test(start_paused = true)]
async fn requested_tools_apply_from_the_next_round() {
    let run = ask(
        vec![
            wants(&[("r1", "request_tools", json!({"bundle": "run_changes"}))]),
            words("Which night works?"),
        ],
        ToolOffer::dynamic([], false),
        "can we move hstar?",
    )
    .await;
    let generation = &run.generation;
    assert_eq!(generation.failure, None, "{:?}", generation.failure);
    // Round 1 offered READ only; the added bundle is offered from round 2.
    assert!(!tool_names(&run.requests[0]).contains(&"propose_move"));
    assert!(tool_names(&run.requests[1]).contains(&"propose_move"));
    assert_eq!(generation.model_rounds[0].bundles, ["read"]);
    assert_eq!(generation.model_rounds[1].bundles, ["read", "run_changes"]);
    assert_eq!(generation.reply, "Which night works?");
    assert_eq!(
        generation.outcomes[0].outcome.output,
        "Added propose_move, propose_add, propose_cancel, propose_rsvp. They are available from your next step."
    );
}

#[tokio::test(start_paused = true)]
async fn request_tools_spends_a_round_of_the_cap() {
    // Four rounds, one spent by `request_tools`: round 2 offers the bundle
    // and round 3 is the (tools-withheld) last.
    let run = run(
        vec![
            wants(&[("r1", "request_tools", json!({"bundle": "strategy"}))]),
            wants(&[("s1", "get_boss_strategy", json!({"boss": "hstar"}))]),
            words("Dodge the lasers."),
        ],
        ToolOffer::dynamic([], false),
        4,
        "how do we do hstar?",
        &Ports::default(),
    )
    .await;
    assert_eq!(run.generation.failure, None);
    assert_eq!(run.generation.rounds, 3);
    assert_eq!(run.requests.len(), 3);
    assert!(tool_names(&run.requests[1]).contains(&"get_boss_strategy"));
    assert!(
        run.requests[2].tools.is_empty(),
        "the charged cap made round 3 last"
    );
    assert_eq!(run.generation.reply, "Dodge the lasers.");
}

/// A card posted beside `request_tools` withholds tools from every later
/// round, so the bundle is never offered and the round is not charged: the
/// question keeps all four rounds.
#[tokio::test(start_paused = true)]
async fn request_tools_beside_a_posted_write_is_not_charged() {
    // Lenient chat validation dispatches these despite no tools being offered.
    let bosses = |id: &str| wants(&[(id, "list_bosses", json!({}))]);
    let run = run(
        vec![
            wants(&[
                (
                    "m1",
                    "propose_move",
                    json!({"run_query": "hstar", "to_when": "thu 22:00"}),
                ),
                ("r1", "request_tools", json!({"bundle": "strategy"})),
            ]),
            bosses("b2"),
            bosses("b3"),
            bosses("b4"),
        ],
        ToolOffer::dynamic([Bundle::RunWrites], false),
        4,
        "move hstar and tell me the strats",
        &Ports::default(),
    )
    .await;
    let generation = &run.generation;
    assert_eq!(generation.posted.len(), 1);
    assert!(generation.outcomes[1].outcome.ok, "the bundle was added");
    assert_eq!(run.requests.len(), 4, "no round was charged");
    assert!(run.requests[1..].iter().all(|r| r.tools.is_empty()));
    assert_eq!(generation.failure, Some(AnswerFailure::KeptCallingTools));
}

/// A request that would leave no round offering the bundle and no final
/// no-tools round is refused (nothing added, nothing charged), and the
/// final round is still sent: in the first of two rounds and in the
/// penultimate default round.
#[tokio::test(start_paused = true)]
async fn request_tools_without_a_round_to_spare_is_refused() {
    let default = usize::from(DEFAULT_TOOL_ROUNDS);
    for (tool_rounds, earlier) in [(2, 0), (DEFAULT_TOOL_ROUNDS, default - 2)] {
        let mut actions: Vec<FakeAction> = (0..earlier)
            .map(|n| wants(&[(&format!("l{n}"), "list_bosses", json!({}))]))
            .collect();
        actions.push(wants(&[(
            "r1",
            "request_tools",
            json!({"bundle": "strategy"}),
        )]));
        actions.push(words("Hard Star, then."));
        let offer = ToolOffer::dynamic([], false);
        let run = run(
            actions,
            offer,
            tool_rounds,
            "how do we do hstar?",
            &Ports::default(),
        )
        .await;
        let (generation, rounds) = (&run.generation, usize::from(tool_rounds));
        assert_eq!(generation.failure, None, "{tool_rounds}");
        let asked = generation.outcomes.last().expect("request_tools outcome");
        assert_eq!(
            (asked.outcome.output.as_str(), asked.outcome.error),
            (NO_ROUND_LEFT, Some(REFUSED)),
            "{tool_rounds}"
        );
        assert_eq!(run.requests.len(), rounds, "{tool_rounds}");
        assert_eq!(generation.rounds as usize, rounds, "{tool_rounds}");
        assert!(run.requests[rounds - 1].tools.is_empty(), "{tool_rounds}");
        let offered = tool_names(&run.requests[rounds - 2]);
        assert!(!offered.contains(&"get_boss_strategy"), "{tool_rounds}");
        assert_eq!(generation.reply, "Hard Star, then.", "{tool_rounds}");
    }
}

/// A call in the same reply as `request_tools` is judged by what that
/// round offered: the new bundle arrives next round.
#[tokio::test(start_paused = true)]
async fn a_tool_requested_in_the_same_reply_waits_for_the_next_round() {
    let run = ask(
        vec![
            wants(&[
                ("r1", "request_tools", json!({"bundle": "run_changes"})),
                ("c1", "propose_cancel", json!({"run_query": "hstar"})),
            ]),
            words("Shall I cancel it?"),
        ],
        ToolOffer::dynamic([], false),
        "cancel hstar",
    )
    .await;
    let cancel = &run.generation.outcomes[1].outcome;
    assert_eq!(cancel.error, Some(REFUSED));
    assert!(
        cancel.output.starts_with(
            "propose_cancel is not available for this message. It becomes available from your next step; call it again then."
        ),
        "{}",
        cancel.output
    );
    assert!(run.generation.created.is_empty(), "nothing was proposed");
    assert!(!tool_names(&run.requests[0]).contains(&"propose_cancel"));
    assert!(tool_names(&run.requests[1]).contains(&"propose_cancel"));
}

/// The question's deadline bounds card posting too; the interaction is
/// still logged, as a timeout with the proposal it created.
#[tokio::test(start_paused = true)]
async fn a_hanging_card_post_ends_the_question_at_its_deadline() {
    let ports = Ports {
        hang: true,
        ..Ports::default()
    };
    let started = tokio::time::Instant::now();
    let actions = vec![move_hstar(), words("never reached")];
    let run = run(actions, ToolOffer::full_set(false), 8, "move hstar", &ports).await;
    let timeout = settings(&run.input, 8).timeout;
    assert!(started.elapsed() >= timeout && started.elapsed() < timeout * 2);
    let generation = &run.generation;
    assert_eq!(
        generation.failure,
        Some(AnswerFailure::Timeout {
            seconds: timeout.as_secs()
        })
    );
    assert_eq!(run.requests.len(), 1);
    assert_eq!(generation.created.len(), 1);
    assert!(generation.posted.is_empty());
    let row = run.row("chat-t", None, 60_000);
    assert_eq!(row.outcome, ChatOutcome::Timeout);
    assert_eq!(row.error.as_deref(), Some("no answer within 60s"));
    // The persisted row still shows the call and the proposal it created.
    assert_eq!(row.rounds[0].tools, ["propose_move"]);
    assert_eq!(
        row.rounds[0].tool_calls[0]["created"],
        json!(generation.created)
    );
}

/// Lenient chat validation hands unknown tools and schema-invalid object
/// arguments to the dispatcher, which answers each as v4 did: an
/// unknown-tool note, or the handler's own refusal for the coerced
/// arguments (v4 read them with `str(args.get(..) or "")`). Nothing fails.
#[tokio::test(start_paused = true)]
async fn unknown_tools_and_schema_invalid_arguments_get_notes_not_failures() {
    let run = ask(
        vec![
            wants(&[
                ("u1", "delete_run", json!({"run_query": "hstar"})),
                ("u2", "get_run", json!({"query": 7})),
                ("u3", "get_schedule", json!({"week": 5})),
                ("u4", "propose_rsvp", json!({"run_query": "hstar"})),
            ]),
            words("Which run did you mean?"),
        ],
        ToolOffer::full_set(false),
        "hmm",
    )
    .await;
    let generation = &run.generation;
    assert_eq!(generation.failure, None);
    let notes: Vec<(&str, Option<&str>)> = generation
        .outcomes
        .iter()
        .map(|o| (o.outcome.output.as_str(), o.outcome.error))
        .collect();
    assert_eq!(
        notes,
        [
            (
                "There is no tool called delete_run. The tools you have are: get_schedule, get_run, list_bosses, get_boss_strategy, get_pending, list_fixed, propose_move, propose_add, propose_cancel, propose_remove_fixed, propose_change_fixed, propose_rsvp. Use one of those when it can answer the request.",
                Some(UNKNOWN)
            ),
            (
                "No run matches `7`. Check what is scheduled, then ask them which one they mean. Do not guess.",
                Some(REFUSED)
            ),
            (
                "Ask whether they mean this week, next week, or a specific day.",
                Some(REFUSED)
            ),
            (
                "answer must be 'yes' or 'no'. Ask them whether they can make it.",
                Some(REFUSED)
            ),
        ]
    );
    // Every call's result reached the model before it answered in words.
    let results = run.requests[1]
        .messages
        .iter()
        .filter(|m| matches!(m, Message::Tool { .. }))
        .count();
    assert_eq!(results, 4);
    assert!(generation.created.is_empty());
    assert_eq!(generation.reply, "Which run did you mean?");
}

/// A call to a tool this question was not offered reaches the dispatcher,
/// which refuses it with the steering note and runs nothing.
#[tokio::test(start_paused = true)]
async fn an_unoffered_tool_is_steered_to_request_tools() {
    let run = ask(
        vec![
            wants(&[("c1", "propose_cancel", json!({"run_query": "hstar"}))]),
            words("Want me to ask for that?"),
        ],
        ToolOffer::dynamic([], false),
        "cancel hstar",
    )
    .await;
    let outcome = &run.generation.outcomes[0].outcome;
    assert_eq!(outcome.error, Some(REFUSED));
    assert!(
        outcome.output.starts_with(
            "propose_cancel is not available for this message. If they asked for that, call request_tools with bundle 'run_changes' first."
        ),
        "{}",
        outcome.output
    );
    assert!(run.generation.created.is_empty(), "nothing was proposed");
    assert!(!tool_names(&run.requests[1]).contains(&"propose_cancel"));
    assert_eq!(run.generation.reply, "Want me to ask for that?");
}

#[tokio::test(start_paused = true)]
async fn an_undelivered_card_is_reported_to_the_model_as_not_posted() {
    let ports = Ports {
        fail: true,
        ..Ports::default()
    };
    let run = run(
        vec![move_hstar(), words("Card's up!")],
        ToolOffer::full_set(false),
        V4_TOOL_ROUNDS,
        "move hstar to thu 22:00",
        &ports,
    )
    .await;
    let generation = &run.generation;
    let outcome = &generation.outcomes[0];
    assert_eq!(
        (outcome.outcome.output.as_str(), outcome.outcome.error),
        (CARD_NOT_POSTED, Some(REFUSED))
    );
    assert_eq!(generation.created.len(), 1, "the proposal exists");
    assert!(generation.posted.is_empty() && generation.focus.is_none());
    // The model read the override, and the next round still offered tools.
    let Message::Tool { content, .. } = &run.requests[1].messages[3] else {
        panic!("tool result");
    };
    assert_eq!(content, CARD_NOT_POSTED);
    assert!(!run.requests[1].tools.is_empty());
    assert!(
        generation
            .reply
            .starts_with("The requested card was not posted.")
    );
    assert_eq!(chat_outcome(generation), ChatOutcome::Refused);
}

/// A schedule lookup, then the hstar move card, then the model's `reply`.
async fn lookup_then_move(reply: &str) -> Run {
    run(
        vec![
            wants(&[("s1", "get_schedule", json!({"scope": "all"}))]),
            move_hstar(),
            words(reply),
        ],
        ToolOffer::full_set(false),
        V4_TOOL_ROUNDS,
        "move hstar to thu 22:00",
        &Ports::default(),
    )
    .await
}

/// `D-GROUND-WRITE`: a lookup then a posted move keeps the model's card
/// reply; v4's regrounding swapped its time line for the whole listing.
#[tokio::test(start_paused = true)]
async fn a_posted_card_reply_is_not_regrounded_into_the_lookup() {
    let said = "The **Hard Star** move is up for *Thu 22:00*. It still needs a ✅ before anything changes.\n\nParty: Alvin tan.";
    let run = lookup_then_move(said).await;
    let generation = &run.generation;
    assert!(generation.outcomes[0].outcome.ok, "the lookup listed runs");
    assert_eq!(generation.posted.len(), 1, "the card was posted");
    let regrounded = shape_reply(said, &generation.tool_outcomes());
    assert_ne!(regrounded, said, "v4 regrounding would swap in the listing");
    assert_eq!(generation.reply, said);
}

/// `D-PERSONAL-CONTEXT` (review note 7): a card-posting turn is not
/// regrounded, but a copied model-only context line never reaches members.
#[tokio::test(start_paused = true)]
async fn a_posted_card_reply_never_carries_copied_context() {
    let said = "The **Hard Star** move is up for *Thu 22:00*. It still needs a ✅ before anything changes.";
    let copied = format!(
        "{said}\nContext (hidden from members): in 2 days · you haven't answered\nin 2 days · you said yes"
    );
    let run = lookup_then_move(&copied).await;
    assert_eq!(run.generation.posted.len(), 1, "the card was posted");
    assert_eq!(run.generation.reply, said);
}

#[tokio::test(start_paused = true)]
async fn a_content_filter_spends_the_clean_retry_then_blocks() {
    let actions = vec![filtered(), words("Here you go.")];
    let answered = ask(actions, ToolOffer::full_set(false), "what's on?").await;
    let generation = &answered.generation;
    assert_eq!(generation.reply, "Here you go.");
    assert!(generation.clean_retry);
    let clean = &answered.requests[1];
    assert!(clean.tools.is_empty(), "the clean retry offers no tools");
    assert_eq!(clean.messages.len(), 3, "system, question, reminder");
    assert_eq!(chat_outcome(generation), ChatOutcome::Answered);

    let actions = vec![filtered(), filtered()];
    let blocked = ask(actions, ToolOffer::full_set(false), "what's on?").await;
    assert_eq!(
        blocked.generation.failure,
        Some(AnswerFailure::ContentBlocked)
    );
    assert!(blocked.generation.clean_retry && blocked.generation.reply.is_empty());
    assert_eq!(
        chat_outcome(&blocked.generation),
        ChatOutcome::ContentBlocked
    );
}

/// An external chat route runs without a permission gate and is labeled.
#[tokio::test(start_paused = true)]
async fn external_chat_rounds_tools_and_clean_retry_send_raw_synthetic_data() {
    let raw = "SyntheticMember 999000111222333444 https://synthetic.invalid/member?id=fixture";
    let question = format!("Check this schedule detail: {raw}");
    let run = run_routed(
        vec![
            wants(&[("g1", "get_run", json!({"query": "hstar"}))]),
            FakeAction::Malformed,
            words("The synthetic schedule detail is noted."),
        ],
        ToolOffer::full_set(false),
        8,
        &question,
        &Ports::default(),
        external,
    )
    .await;
    assert!(run.generation.failure.is_none());
    assert!(run.generation.clean_retry);
    assert_eq!(run.requests.len(), 3);
    assert_eq!(run.generation.route(), "external_unmasked");
    assert!(run.generation.external_unmasked);
    for request in [&run.requests[0], run.requests.last().unwrap()] {
        let serialized = serde_json::to_string(&request.messages).unwrap();
        assert!(serialized.contains("SyntheticMember"));
        assert!(serialized.contains("999000111222333444"));
        assert!(serialized.contains("https://synthetic.invalid/member?id=fixture"));
    }
    let tool_result = run.requests[1]
        .messages
        .iter()
        .find_map(|message| match message {
            Message::Tool { content, .. } => Some(content),
            _ => None,
        })
        .expect("tool result is in the next raw round");
    assert_eq!(tool_result, &run.generation.outcomes[0].outcome.output);
    let row = run.row("chat-raw", None, 1);
    assert_eq!(row.guardrail, json!({"external_unmasked": true}));
}

#[tokio::test(start_paused = true)]
async fn an_external_pre_admission_refusal_does_not_claim_disclosure() {
    use kanade::infrastructure::llm::governor::{Refused, SessionError, SessionFailure};

    let input = world_input();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new([]),
        caps: capabilities(&input["caps"]),
    });
    let (governor, client) = client(Some(MODEL), provider.clone());
    assert!(governor.set_external(Role::Chat, true));
    let mut route = governor.route(Role::Chat).expect("chat route");
    route.group = None;
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let persona = kanade();
    let deps = AnswerDeps {
        client: &client,
        route: Some(&route),
    };
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation: conversation("synthetic question"),
            profanity: None,
            reminder: persona.voice_reminder(),
            offer: ToolOffer::dynamic([], false),
            settings: settings(&input, 8),
        };
        answer(&deps, question, &guild, &mut proposer, &Ports::default()).await
    };

    assert!(matches!(
        generation.failure,
        Some(AnswerFailure::Session(SessionError {
            failure: SessionFailure::Refused(Refused::Ungrouped),
            ..
        }))
    ));
    assert_eq!(generation.requests, 0);
    assert!(generation.external, "the selected route stays external");
    assert!(!generation.external_unmasked);
    assert!(provider.fake.requests().is_empty());
    let row = interaction(
        "chat-pre-admission".into(),
        world.clock.now().with_timezone(&Utc),
        &ctx,
        "synthetic question",
        &generation,
        MODEL,
        None,
        0,
    );
    assert!(row.guardrail.get("external_unmasked").is_none());
}

#[tokio::test(start_paused = true)]
async fn an_external_admitted_failure_keeps_the_disclosure_guardrail() {
    let run = run_routed(
        vec![filtered(), filtered()],
        ToolOffer::dynamic([], false),
        8,
        "synthetic external failure",
        &Ports::default(),
        external,
    )
    .await;
    assert_eq!(run.generation.failure, Some(AnswerFailure::ContentBlocked));
    assert!(run.generation.requests > 0);
    assert!(!run.requests.is_empty());
    assert!(run.generation.external_unmasked);

    let row = run.row("chat-admitted-failure", None, 1);
    assert_eq!(
        row.guardrail,
        json!({"content_filter": true, "external_unmasked": true})
    );
}

/// `list_fixed`, then "Three weeklies." (as `first`'s reply when given).
async fn weeklies(first: Option<FakeAction>, offer: ToolOffer) -> Run {
    let first = first.unwrap_or_else(|| wants(&[("l1", "list_fixed", json!({}))]));
    ask(
        vec![first, words("Three weeklies.")],
        offer,
        "which weeklies?",
    )
    .await
}

#[tokio::test(start_paused = true)]
async fn a_question_is_logged_as_one_interaction_with_its_rounds() {
    let run = weeklies(None, ToolOffer::dynamic([Bundle::Strategy], false)).await;
    let row = stored_row(&run, "chat-1").await;
    assert_eq!(row.outcome, ChatOutcome::Answered);
    assert_eq!(row.request_count, 2);
    assert_eq!(row.rounds.len(), 2);
    assert_eq!(row.rounds[0].tools, ["list_fixed"]);
    assert_eq!(row.rounds[0].tool_bundles, ["read", "strategy"]);
    assert_eq!(row.rounds[0].tool_calls[0]["outcome"], "ok");
    assert_eq!(row.rounds[1].response.as_deref(), Some("Three weeklies."));
}

/// `action`'s response with a reported usage pair.
fn reported(action: FakeAction, prompt_tokens: u32, completion_tokens: u32) -> FakeAction {
    let FakeAction::Response(mut response) = action else {
        panic!("a response");
    };
    response.usage = Some(kanade::infrastructure::llm::Usage {
        prompt_tokens,
        completion_tokens,
    });
    FakeAction::Response(response)
}

/// The budget's estimate for a request as sent: its messages without the
/// trailing voice reminder, re-budgeted untrimmed with its tool schemas.
fn estimate_of(request: &ChatRequest) -> Option<u64> {
    use kanade::chat::context::budgeted;
    use kanade::chat::tools::ToolName;
    use kanade::chat::tools::schemas::surface_text;
    let mut messages = request.messages.clone();
    let Some(Message::User { content: reminder }) = messages.pop() else {
        panic!("the reminder is last");
    };
    let tools: Vec<ToolName> = request
        .tools
        .iter()
        .map(|tool| ToolName::parse(&tool.name).expect("known tool"))
        .collect();
    let fits = budgeted(
        &mut messages,
        &surface_text(&tools),
        &reminder,
        usize::MAX,
        0,
        "",
    )
    .expect("fits untrimmed");
    Some(u64::try_from(fits.estimate).expect("fits"))
}

/// Per-round usage (D-USAGE-PAIRS keeps the turn totals): each answered
/// request logs its own reported pair and its budget estimate without the
/// completion reserve; a round that reported nothing keeps an unset pair.
#[tokio::test(start_paused = true)]
async fn each_round_logs_its_reported_pair_and_prompt_estimate() {
    let first = reported(wants(&[("l1", "list_fixed", json!({}))]), 900, 20);
    let run = weeklies(Some(first), ToolOffer::dynamic([Bundle::Strategy], false)).await;
    let row = stored_row(&run, "chat-usage").await;
    assert_eq!(run.requests.len(), 2);
    let usage: Vec<_> = row
        .rounds
        .iter()
        .map(|round| {
            (
                round.prompt_tokens,
                round.completion_tokens,
                round.prompt_estimate,
            )
        })
        .collect();
    assert_eq!(
        usage,
        [
            (Some(900), Some(20), estimate_of(&run.requests[0])),
            (None, None, estimate_of(&run.requests[1])),
        ]
    );
    let settings = settings(&run.input, 8);
    assert!(
        row.rounds[0].prompt_estimate.expect("estimated") + u64::from(settings.max_output_tokens)
            <= settings.model_context_tokens as u64,
        "the estimate leaves the completion reserve out"
    );
    assert_eq!(
        (row.prompt_tokens, row.completion_tokens),
        (Some(900), Some(20)),
        "turn totals sum the rounds that reported"
    );
}

#[tokio::test(start_paused = true)]
async fn reasoning_is_logged_per_round_but_never_enters_the_tool_loop() {
    use kanade::domain::model_log::{REASONING_CAP, REASONING_TRUNCATED};
    use kanade::infrastructure::llm::wire_body;
    let FakeAction::Response(mut first) = wants(&[("r1", "list_fixed", json!({}))]) else {
        panic!("response")
    };
    first.reasoning_content = Some("奏".repeat(REASONING_CAP));
    first.reasoning_tokens = Some(32);
    let first = FakeAction::Response(first);
    let run = weeklies(Some(first), ToolOffer::dynamic([Bundle::Strategy], false)).await;
    let row = stored_row(&run, "chat-reasoning").await;
    assert_eq!(row.rounds[0].reasoning_tokens, Some(32));
    let text = row.rounds[0].reasoning_content.as_deref().expect("text");
    assert!(text.len() <= REASONING_CAP && text.ends_with(REASONING_TRUNCATED));
    assert_eq!(
        (
            row.rounds[1].reasoning_content.as_ref(),
            row.rounds[1].reasoning_tokens
        ),
        (None, None)
    );
    assert_eq!(run.requests.len(), 2);
    for request in &run.requests {
        let body = wire_body(request, &capabilities(&run.input["caps"])).expect("body");
        for message in body["messages"].as_array().expect("messages") {
            assert!(message.as_object().expect("message").keys().all(|key| {
                ["role", "content", "tool_calls", "tool_call_id"].contains(&key.as_str())
            }));
            assert!(!message.to_string().contains('奏'));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn the_clean_retry_is_a_round_with_its_own_usage() {
    let actions = vec![filtered(), reported(words("Here you go."), 300, 12)];
    let run = ask(actions, ToolOffer::full_set(false), "what's on?").await;
    let rounds = &run.generation.model_rounds;
    assert_eq!(rounds.len(), 1, "the filtered request was never answered");
    assert!(rounds[0].clean);
    assert_eq!(
        (
            rounds[0].prompt_tokens,
            rounds[0].completion_tokens,
            rounds[0].prompt_estimate
        ),
        (Some(300), Some(12), estimate_of(&run.requests[1]))
    );
    assert_eq!(
        (
            run.generation.prompt_tokens,
            run.generation.completion_tokens
        ),
        (Some(300), Some(12))
    );
}

/// The chat-log row of `run`, stored and read back.
async fn stored_row(run: &Run, id: &str) -> ChatInteraction {
    let row = run.row(id, settings(&run.input, 8).reasoning, 1);
    let store = run.world.service.store();
    store.record_chat(row.clone()).await.expect("record");
    let stored = store.load_chat(id).await.expect("load").expect("row");
    assert_eq!(stored, row);
    stored
}

/// `{session}-1..=n`, the ids a session numbers its requests with.
fn numbered(session: &str, n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("{session}-{i}")).collect()
}

#[tokio::test(start_paused = true)]
async fn logged_request_ids_are_exactly_the_ids_sent_across_rounds_and_retries() {
    use kanade::infrastructure::llm::LlmProvider as _;
    // Round 1 answers at once; round 2's first request fails transiently and
    // its retry answers.
    let run = ask(
        vec![
            wants(&[("r1", "list_fixed", json!({}))]),
            FakeAction::Transient,
            words("Three weeklies."),
        ],
        ToolOffer::dynamic([Bundle::Strategy], false),
        "which weeklies?",
    )
    .await;
    let row = stored_row(&run, "chat-rounds").await;
    let session = row.session_id.clone().expect("a tagged request went out");
    assert!(session.starts_with("kanade-chat-"), "{session}");
    let sent = &run.request_ids;
    assert_eq!(*sent, numbered(&session, 3));
    assert_eq!(run.generation.request_ids, *sent);
    assert_eq!(
        row.rounds
            .iter()
            .map(|round| round.request_ids.clone())
            .collect::<Vec<_>>(),
        [sent[..1].to_vec(), sent[1..].to_vec()]
    );

    // A filtered request answers no round, so only the session stem finds
    // it; the clean retry is its own round.
    let actions = vec![filtered(), words("Here you go.")];
    let run = ask(actions, ToolOffer::full_set(false), "which weeklies?").await;
    let row = stored_row(&run, "chat-clean").await;
    let session = row.session_id.clone().expect("session");
    assert_eq!(run.request_ids, numbered(&session, 2));
    // The filtered request is in no round, but the session list keeps it.
    assert_eq!(run.generation.request_ids, run.request_ids);
    assert_eq!(row.rounds.len(), 1);
    assert!(row.rounds[0].clean);
    assert_eq!(row.rounds[0].request_ids, run.request_ids[1..]);

    // An untagged call sends no id.
    let plain = FakeProvider::new(Vec::new());
    let _ = plain.complete(&run.requests[0]).await;
    assert!(plain.request_ids().is_empty());
}

/// Ports whose pending-card read takes a fixed time, so a call's wall time
/// is known under the paused clock.
#[derive(Default)]
struct SlowPorts(Ports);

impl kanade::chat::answer::ChatPorts for SlowPorts {
    async fn refresh_proposals(&self, proposal_ids: &[String]) {
        self.0.refresh_proposals(proposal_ids).await;
    }

    async fn pending(&self) -> Vec<kanade::chat::tools::read::PendingCard> {
        tokio::time::sleep(Duration::from_millis(250)).await;
        self.0.pending().await
    }

    async fn post_card(&self, card: &kanade::chat::tools::ProposalCard) -> Result<(), String> {
        self.0.post_card(card).await
    }
}

#[tokio::test(start_paused = true)]
async fn a_logged_call_keeps_its_result_and_wall_time_through_the_store_and_the_turn() {
    use kanade::api::dto::logs::{Names, chat_turn};

    let mut run = run(
        vec![
            wants(&[("l1", "list_fixed", json!({}))]),
            words("Three weeklies."),
        ],
        ToolOffer::full_set(false),
        8,
        "which weeklies?",
        &SlowPorts::default(),
    )
    .await;
    let output = run.generation.outcomes[0].outcome.output.clone();
    assert!(!output.is_empty());
    let took = run.generation.outcomes[0].took_ms;
    assert!(took >= 250, "{took}");
    assert_eq!(run.generation.tools_ms, took);

    let row = run.row("chat-t", None, 10);
    assert_eq!(row.rounds[0].tool_calls[0]["result"], output.as_str());
    assert_eq!(row.rounds[0].tool_calls[0]["took_ms"], took);
    let store = run.world.service.store();
    store.record_chat(row.clone()).await.expect("recorded");
    let stored = store.load_chat("chat-t").await.expect("load").expect("row");
    assert_eq!(stored, row);
    let roster = kanade::domain::members::Roster::new();
    let channels = std::collections::BTreeMap::new();
    let names = Names {
        roster: &roster,
        channels: &channels,
    };
    let turn = chat_turn(&names, &stored, &[], None, None);
    assert_eq!(turn["tools"][0]["result"], output.as_str());
    assert_eq!(turn["tools"][0]["took_ms"], took);

    // 8 KiB are kept, cut before a two-byte char that straddles the cap.
    let long = format!("a{}", "é".repeat(5000));
    run.generation.outcomes[0].outcome.output.clone_from(&long);
    let result = run.row("chat-t", None, 10).rounds[0].tool_calls[0]["result"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        result,
        format!("{}… [truncated, 10001 bytes]", &long[..8191])
    );
    let exact = "b".repeat(8192);
    run.generation.outcomes[0].outcome.output.clone_from(&exact);
    assert_eq!(
        run.row("chat-t", None, 10).rounds[0].tool_calls[0]["result"],
        exact.as_str()
    );
}

/// A deadline that passes while a proposal is being staged never cuts the
/// staging short: the committed proposal is reported, the older card it
/// replaces is retired, nothing is posted after the deadline, and the
/// question ends as a timeout.
#[tokio::test(start_paused = true)]
async fn a_deadline_during_staging_still_reports_and_supersedes_the_proposal() {
    const HSTAR: &str = "a1a1a1a1-0000-4000-8000-000000000001";
    let input = world_input();
    let mut world = World::new(&input).await;
    let timeout = settings(&input, 8).timeout;

    let mut older = ProposedChange::new(ChangeKind::Cancel);
    older.run_id = Some(HSTAR.into());
    older.channel_id = Some("900".into());
    older.bosses = vec!["HMaleficStar".into(), "HFA".into()];
    let older = world
        .service
        .propose(
            ProposalRequest {
                change: older,
                source: ProposalSource::Chat,
                source_id: "older".into(),
                supersede: Supersede::Keep,
            },
            &world.policy,
            &world.guild,
        )
        .await
        .expect("an older live card")
        .proposal
        .id;

    let mut slow = SchedulerService::new(
        SlowStore {
            inner: world.service.store(),
            stall: timeout + Duration::from_secs(1),
        },
        world.ids.clone(),
        world.clock.clone(),
    );
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new([move_hstar(), words("never reached")]),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let ports = Ports::default();
    let question = Question {
        ctx: &ctx,
        conversation: conversation("move hstar to thu 22:00"),
        profanity: None,
        reminder: kanade().voice_reminder(),
        offer: ToolOffer::full_set(false),
        settings: settings(&input, 8),
    };
    let guild = world.guild_view();
    let mut proposer = Proposer {
        service: &mut slow,
        policy: &world.policy,
    };
    let generation = answer(&deps, question, &guild, &mut proposer, &ports).await;

    assert_eq!(
        generation.failure,
        Some(AnswerFailure::Timeout {
            seconds: timeout.as_secs()
        })
    );
    assert_eq!(
        provider.fake.requests().len(),
        1,
        "no round after the deadline"
    );
    let store = world.service.store();
    let [created] = generation.created.as_slice() else {
        panic!(
            "the committed proposal is reported: {:?}",
            generation.created
        );
    };
    let status = async |id: &str| {
        let (loaded, _) = store
            .load_proposal(id)
            .await
            .expect("load")
            .expect("stored");
        (loaded.draft.status, loaded.draft.close_reason)
    };
    assert_eq!(status(created).await.0, DraftStatus::Submitted);
    assert_eq!(
        status(&older).await,
        (DraftStatus::Discarded, Some(SUPERSEDED.to_owned())),
        "supersede ran"
    );
    assert!(ports.posted.lock().unwrap().is_empty() && generation.posted.is_empty());
    let staged = &generation.outcomes[0];
    assert!(staged.outcome.ok && staged.posted.is_empty());
    assert_eq!(staged.outcome.created, generation.created);
    let row = interaction(
        "chat-d".into(),
        world.clock.now().with_timezone(&Utc),
        &ctx,
        "move hstar to thu 22:00",
        &generation,
        MODEL,
        None,
        61_000,
    );
    assert_eq!(row.outcome, ChatOutcome::Timeout);
    assert_eq!(
        row.rounds[0].tool_calls[0]["created"],
        json!(generation.created)
    );
}
