//! The v5 question loop beyond v4's vectors: dynamic tool rounds and
//! `request_tools`, card delivery failure, the clean retry on a content
//! filter, identity encoding across rounds, and the chat-log row.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use kanade::chat::answer::{
    AnswerDeps, AnswerFailure, CARD_NOT_POSTED, Generation, Question, answer, chat_outcome,
    interaction,
};
use kanade::chat::tools::bundles::{Bundle, NO_ROUND_LEFT, ToolOffer};
use kanade::chat::tools::propose::Proposer;
use kanade::chat::tools::{REFUSED, ToolContext, UNKNOWN};
use kanade::domain::drafts::{DraftStatus, ProposalSource, ProposalStore, SUPERSEDED};
use kanade::domain::model_log::{ChatOutcome, ModelLogStore};
use kanade::domain::proposals::{ChangeKind, ProposedChange};
use kanade::domain::scheduler::Clock;
use kanade::domain::scheduler::{ProposalRequest, SchedulerService, Supersede};
use kanade::infrastructure::llm::governor::{DEFAULT_TOOL_ROUNDS, Governor};
use kanade::infrastructure::llm::identity::{
    IdentityCodec, Passthrough, TaggingCodec, find_request_leaks,
};
use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ToolCall,
};
use serde_json::{Value, json};

use crate::looping::{Ports, V4_TOOL_ROUNDS, roster, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::slow_store::SlowStore;
use crate::support::load;
use crate::wire::kanade;
use crate::world::World;

const MODEL: &str = "synthetic-chat";

fn wants(calls: &[(&str, &str, Value)]) -> FakeAction {
    FakeAction::Response(CompletionResponse {
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

fn filtered() -> FakeAction {
    FakeAction::Response(CompletionResponse {
        model: MODEL.into(),
        content: None,
        tool_calls: Vec::new(),
        finish_reason: FinishReason::ContentFilter,
        usage: None,
    })
}

struct Run {
    generation: Generation,
    requests: Vec<ChatRequest>,
    world: World,
    input: Value,
    ctx: ToolContext,
    _governor: Arc<Governor>,
}

async fn run(
    actions: Vec<FakeAction>,
    offer: ToolOffer,
    tool_rounds: u8,
    question: &str,
    codec: &dyn IdentityCodec,
    ports: &Ports,
) -> Run {
    let input = load("loop.json")["cases"][0]["input"].clone();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (governor, client) = client(Some(MODEL), provider.clone());
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let persona = kanade();
    let roster = roster(&world);
    let deps = AnswerDeps {
        client: &client,
        codec,
        roster: &roster,
    };
    let conversation = vec![
        Message::System {
            content: "SYSTEM".into(),
        },
        Message::User {
            content: format!("Alvin tan: {question}"),
        },
    ];
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation,
            reminder: persona.voice_reminder(),
            offer,
            settings: settings(&input, tool_rounds),
        };
        answer(&deps, question, &guild, &mut proposer, ports).await
    };
    Run {
        generation,
        requests: provider.fake.requests(),
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
    let ports = Ports::default();
    let run = run(
        vec![
            wants(&[("r1", "request_tools", json!({"bundle": "run_changes"}))]),
            words("Which night works?"),
        ],
        ToolOffer::dynamic([], false),
        8,
        "can we move hstar?",
        &Passthrough,
        &ports,
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
        &Passthrough,
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
        &Passthrough,
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
/// final round is still sent.
async fn refused_without_room(tool_rounds: u8, earlier: usize) -> Run {
    let mut actions: Vec<FakeAction> = (0..earlier)
        .map(|n| wants(&[(&format!("l{n}"), "list_bosses", json!({}))]))
        .collect();
    actions.push(wants(&[(
        "r1",
        "request_tools",
        json!({"bundle": "strategy"}),
    )]));
    actions.push(words("Hard Star, then."));
    run(
        actions,
        ToolOffer::dynamic([], false),
        tool_rounds,
        "how do we do hstar?",
        &Passthrough,
        &Ports::default(),
    )
    .await
}

fn assert_refused_then_answered(run: &Run, tool_rounds: usize) {
    let generation = &run.generation;
    assert_eq!(generation.failure, None, "{:?}", generation.failure);
    let asked = generation.outcomes.last().expect("request_tools outcome");
    assert_eq!(
        (asked.outcome.output.as_str(), asked.outcome.error),
        (NO_ROUND_LEFT, Some(REFUSED))
    );
    assert_eq!(run.requests.len(), tool_rounds);
    assert_eq!(generation.rounds as usize, tool_rounds);
    assert!(run.requests[tool_rounds - 1].tools.is_empty());
    assert!(!tool_names(&run.requests[tool_rounds - 2]).contains(&"get_boss_strategy"));
    assert_eq!(generation.reply, "Hard Star, then.");
}

#[tokio::test(start_paused = true)]
async fn request_tools_in_the_first_of_two_rounds_is_refused() {
    let run = refused_without_room(2, 0).await;
    assert_refused_then_answered(&run, 2);
}

#[tokio::test(start_paused = true)]
async fn request_tools_in_the_penultimate_default_round_is_refused() {
    let rounds = usize::from(DEFAULT_TOOL_ROUNDS);
    let run = refused_without_room(DEFAULT_TOOL_ROUNDS, rounds - 2).await;
    assert_refused_then_answered(&run, rounds);
}

/// A call in the same reply as `request_tools` is judged by what that
/// round offered: the new bundle arrives next round.
#[tokio::test(start_paused = true)]
async fn a_tool_requested_in_the_same_reply_waits_for_the_next_round() {
    let run = run(
        vec![
            wants(&[
                ("r1", "request_tools", json!({"bundle": "run_changes"})),
                ("c1", "propose_cancel", json!({"run_query": "hstar"})),
            ]),
            words("Shall I cancel it?"),
        ],
        ToolOffer::dynamic([], false),
        8,
        "cancel hstar",
        &Passthrough,
        &Ports::default(),
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
    let run = run(
        vec![
            wants(&[(
                "m1",
                "propose_move",
                json!({"run_query": "hstar", "to_when": "thu 22:00"}),
            )]),
            words("never reached"),
        ],
        ToolOffer::full_set(false),
        8,
        "move hstar",
        &Passthrough,
        &ports,
    )
    .await;
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
    let at = run.world.clock.now().with_timezone(&Utc);
    let row = interaction(
        "chat-t".into(),
        at,
        &run.ctx,
        "move hstar",
        generation,
        MODEL,
        None,
        60_000,
    );
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
    let run = run(
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
        8,
        "hmm",
        &Passthrough,
        &Ports::default(),
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
    let run = run(
        vec![
            wants(&[("c1", "propose_cancel", json!({"run_query": "hstar"}))]),
            words("Want me to ask for that?"),
        ],
        ToolOffer::dynamic([], false),
        8,
        "cancel hstar",
        &Passthrough,
        &Ports::default(),
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
        vec![
            wants(&[(
                "m1",
                "propose_move",
                json!({"run_query": "hstar", "to_when": "thu 22:00"}),
            )]),
            words("Card's up!"),
        ],
        ToolOffer::full_set(false),
        V4_TOOL_ROUNDS,
        "move hstar to thu 22:00",
        &Passthrough,
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

#[tokio::test(start_paused = true)]
async fn a_content_filter_spends_the_clean_retry_then_blocks() {
    let answered = run(
        vec![filtered(), words("Here you go.")],
        ToolOffer::full_set(false),
        8,
        "what's on?",
        &Passthrough,
        &Ports::default(),
    )
    .await;
    let generation = &answered.generation;
    assert_eq!(generation.reply, "Here you go.");
    assert!(generation.clean_retry);
    let clean = &answered.requests[1];
    assert!(clean.tools.is_empty(), "the clean retry offers no tools");
    assert_eq!(clean.messages.len(), 3, "system, question, reminder");
    assert_eq!(chat_outcome(generation), ChatOutcome::Answered);

    let blocked = run(
        vec![filtered(), filtered()],
        ToolOffer::full_set(false),
        8,
        "what's on?",
        &Passthrough,
        &Ports::default(),
    )
    .await;
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

#[tokio::test(start_paused = true)]
async fn identities_are_encoded_in_every_request_and_decoded_in_the_reply() {
    let run = run(
        vec![
            wants(&[("g1", "get_run", json!({"query": "hstar"}))]),
            words("ID1 and ID2 are on it."),
        ],
        ToolOffer::full_set(false),
        8,
        "who is on hstar with kanon?",
        &TaggingCodec,
        &Ports::default(),
    )
    .await;
    let roster = roster(&run.world);
    for request in &run.requests {
        assert_eq!(find_request_leaks(request, &roster), Vec::<String>::new());
    }
    assert_eq!(
        run.generation.reply,
        "Alvin tan and kanon [AZUR] are on it."
    );

    let unknown = crate::answer::run(
        vec![words("ID9 is on it."), words("Nobody I know.")],
        ToolOffer::full_set(false),
        8,
        "who?",
        &TaggingCodec,
        &Ports::default(),
    )
    .await;
    assert!(
        unknown.generation.clean_retry,
        "an unknown token is a malformed answer"
    );
    assert_eq!(unknown.generation.reply, "Nobody I know.");
}

#[tokio::test(start_paused = true)]
async fn a_question_is_logged_as_one_interaction_with_its_rounds() {
    let run = run(
        vec![
            wants(&[("l1", "list_fixed", json!({}))]),
            words("Three weeklies."),
        ],
        ToolOffer::dynamic([Bundle::Strategy], false),
        8,
        "which weeklies?",
        &Passthrough,
        &Ports::default(),
    )
    .await;
    let at = run.world.clock.now().with_timezone(&Utc);
    let row = interaction(
        "chat-1".into(),
        at,
        &run.ctx,
        "which weeklies?",
        &run.generation,
        MODEL,
        settings(&run.input, 8).reasoning,
        1234,
    );
    assert_eq!(row.outcome, ChatOutcome::Answered);
    assert_eq!(row.request_count, 2);
    assert_eq!(row.rounds.len(), 2);
    assert_eq!(row.rounds[0].tools, ["list_fixed"]);
    assert_eq!(row.rounds[0].tool_bundles, ["read", "strategy"]);
    assert_eq!(row.rounds[0].tool_calls[0]["outcome"], "ok");
    assert_eq!(row.rounds[1].response.as_deref(), Some("Three weeklies."));
    let store = run.world.service.store();
    store.record_chat(row.clone()).await.expect("recorded");
    assert_eq!(store.load_chat("chat-1").await.expect("load"), Some(row));
}

/// A deadline that passes while a proposal is being staged never cuts the
/// staging short: the committed proposal is reported, the older card it
/// replaces is retired, nothing is posted after the deadline, and the
/// question ends as a timeout.
#[tokio::test(start_paused = true)]
async fn a_deadline_during_staging_still_reports_and_supersedes_the_proposal() {
    const HSTAR: &str = "a1a1a1a1-0000-4000-8000-000000000001";
    let input = load("loop.json")["cases"][0]["input"].clone();
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
        fake: FakeProvider::new([
            wants(&[(
                "m1",
                "propose_move",
                json!({"run_query": "hstar", "to_when": "thu 22:00"}),
            )]),
            words("never reached"),
        ]),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let ctx = world.context(&json!({"author_id": "11", "channel_id": "900"}));
    let roster = roster(&world);
    let deps = AnswerDeps {
        client: &client,
        codec: &Passthrough,
        roster: &roster,
    };
    let ports = Ports::default();
    let question = Question {
        ctx: &ctx,
        conversation: vec![
            Message::System {
                content: "SYSTEM".into(),
            },
            Message::User {
                content: "Alvin tan: move hstar to thu 22:00".into(),
            },
        ],
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
    let (new, _) = store
        .load_proposal(created)
        .await
        .expect("load")
        .expect("stored");
    assert_eq!(new.draft.status, DraftStatus::Submitted);
    let (old, _) = store
        .load_proposal(&older)
        .await
        .expect("load")
        .expect("stored");
    assert_eq!(
        (old.draft.status, old.draft.close_reason.as_deref()),
        (DraftStatus::Discarded, Some(SUPERSEDED)),
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
