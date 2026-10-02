//! The per-request context budget inside the question loop: the route's
//! resolved completion reserve decides the fit, and an over-budget question
//! is a typed failure with a member reply and an identifying log row.

use std::sync::Arc;

use chrono::Utc;
use kanade::chat::answer::{
    AnswerDeps, AnswerFailure, CONTEXT_BUDGET_REPLY, Generation, Question, answer, chat_outcome,
    interaction,
};
use kanade::chat::context::ContextBudgetError;
use kanade::chat::tools::ToolContext;
use kanade::chat::tools::bundles::{Bundle, ToolOffer};
use kanade::domain::model_log::ChatOutcome;
use kanade::domain::scheduler::Clock;
use kanade::infrastructure::llm::{
    CompletionResponse, FakeAction, FakeProvider, FinishReason, Message, ToolCall,
};

use crate::looping::{Ports, said, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::wire::kanade;
use crate::world::World;

const MODEL: &str = "synthetic-chat";
const QUESTION: &str = "what's on tonight?";

struct Asked {
    generation: Generation,
    sent: usize,
    ctx: ToolContext,
    world: World,
}

/// One question with the given resolved window and completion reserve.
async fn ask(window: usize, reserve: u32) -> Asked {
    ask_scripted(
        window,
        reserve,
        vec![FakeAction::Response(said(MODEL, "Nothing on."))],
        ToolOffer::dynamic([], false),
        &Ports::default(),
    )
    .await
}

async fn ask_scripted(
    window: usize,
    reserve: u32,
    actions: Vec<FakeAction>,
    offer: ToolOffer,
    ports: &Ports,
) -> Asked {
    let input = load("loop.json")["cases"][0]["input"].clone();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(actions),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let ctx = world.context(&serde_json::json!({"author_id": "11", "channel_id": "900"}));
    let mut tuned = settings(&input, 8);
    tuned.model_context_tokens = window;
    tuned.max_output_tokens = reserve;
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ctx,
            conversation: vec![
                Message::System {
                    content: "SYSTEM".into(),
                },
                Message::User {
                    content: format!("Alvin tan: {QUESTION}"),
                },
            ],
            reminder: kanade().voice_reminder(),
            offer,
            settings: tuned,
        };
        let deps = AnswerDeps {
            client: &client,
            route: None,
        };
        answer(&deps, question, &guild, &mut proposer, ports).await
    };
    Asked {
        generation,
        sent: provider.fake.requests().len(),
        ctx,
        world,
    }
}

fn budget_error(generation: &Generation) -> ContextBudgetError {
    match &generation.failure {
        Some(AnswerFailure::ContextBudget(error)) => *error,
        other => panic!("expected a context budget failure, got {other:?}"),
    }
}

/// The request's own estimate: what a window of zero reports, less the reserve.
async fn request_estimate() -> usize {
    let probe = ask(0, 0).await;
    budget_error(&probe.generation).estimate
}

#[tokio::test(start_paused = true)]
async fn the_resolved_reserve_not_the_old_constant_decides_the_fit() {
    let request = request_estimate().await;
    // Room for the old 1024-token reserve, but not for this route's 3000.
    let window = request + 1024;
    let over = ask(window, 3000).await;
    assert_eq!(
        budget_error(&over.generation),
        ContextBudgetError {
            estimate: request + 3000,
            budget: window,
            reserve: 3000,
        }
    );
    assert_eq!(over.sent, 0, "nothing reaches the model");

    let fits = ask(window, 1024).await;
    assert_eq!(fits.generation.failure, None);
    assert_eq!(fits.sent, 1);
    let smaller = ask(request + 10, 10).await;
    assert_eq!(
        smaller.generation.failure, None,
        "a small reserve fits a small window"
    );
}

#[tokio::test(start_paused = true)]
async fn an_over_budget_question_is_typed_replied_and_logged() {
    let request = request_estimate().await;
    let asked = ask(request, 512).await;
    let generation = &asked.generation;
    let error = budget_error(generation);
    assert_eq!((error.budget, error.reserve), (request, 512));
    assert_eq!(generation.reply, CONTEXT_BUDGET_REPLY);
    assert_eq!(asked.sent, 0);
    assert_eq!(chat_outcome(generation), ChatOutcome::Error);

    let row = interaction(
        "chat-budget".into(),
        asked.world.clock.now().with_timezone(&Utc),
        &asked.ctx,
        QUESTION,
        generation,
        MODEL,
        None,
        0,
    );
    assert_eq!(row.error_code.as_deref(), Some("context_budget"));
    assert_eq!(row.reply, CONTEXT_BUDGET_REPLY);
    assert_eq!(
        row.error.as_deref(),
        Some(
            format!(
                "ContextBudgetError: chat request estimate {} exceeds context budget {request} with completion reserve 512",
                request + 512
            )
            .as_str()
        )
    );
}

/// A card posted in round 1 is recorded; a later round that overflows must
/// not tell the member to shorten and resend, which would duplicate it.
#[tokio::test(start_paused = true)]
async fn an_overflow_after_a_posted_card_does_not_invite_a_resend() {
    // Oversized call arguments (ignored by the handler) fit round 1's
    // request but push round 2's, which resends them, over the window.
    let padding: String = std::iter::repeat_n('p', 60_000).collect();
    let arguments =
        serde_json::json!({"run_query": "hstar", "to_when": "thu 22:00", "note": padding});
    let propose = || {
        FakeAction::Response(CompletionResponse {
            reasoning_content: None,
            reasoning_tokens: None,
            model: MODEL.into(),
            content: None,
            tool_calls: vec![ToolCall {
                id: "m1".into(),
                name: "propose_move".into(),
                arguments: arguments.to_string(),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        })
    };
    let offer = || ToolOffer::dynamic([Bundle::RunWrites], false);
    let first_round = {
        let probe = ask_scripted(0, 0, vec![propose()], offer(), &Ports::default()).await;
        budget_error(&probe.generation).estimate
    };
    let ports = Ports::default();
    let asked = ask_scripted(
        first_round + 512,
        512,
        vec![propose(), FakeAction::Response(said(MODEL, "Moved."))],
        offer(),
        &ports,
    )
    .await;
    let generation = &asked.generation;
    assert_eq!(asked.sent, 1, "round 2 never reached the model");
    assert_eq!(generation.posted.len(), 1);
    assert_eq!(ports.posted.lock().unwrap().len(), 1);
    assert_eq!(budget_error(generation).reserve, 512);
    assert_eq!(
        generation.reply,
        "The requested card was posted, but the request did not finish cleanly."
    );
    assert_ne!(generation.reply, CONTEXT_BUDGET_REPLY);
    let row = interaction(
        "chat-budget-posted".into(),
        asked.world.clock.now().with_timezone(&Utc),
        &asked.ctx,
        QUESTION,
        generation,
        MODEL,
        None,
        0,
    );
    assert_eq!(row.error_code.as_deref(), Some("context_budget"));
    assert_eq!(row.reply, generation.reply);
}
