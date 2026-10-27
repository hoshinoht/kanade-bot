//! Shared drivers for `answer` (identity session, governed question session,
//! real runner, fake provider): recording chat ports, the world fixture's
//! settings, and one full-set question.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use kanade_chat::answer::{AnswerDeps, AnswerSettings, ChatPorts, Generation, Question, answer};
use kanade_chat::context::COMPLETION_RESERVE_TOKENS;
use kanade_chat::tools::bundles::ToolOffer;
use kanade_chat::tools::read::PendingCard;
use kanade_chat::tools::{ProposalCard, ToolContext};
use kanade_llm::{CompletionResponse, FakeAction, FinishReason, Message};
use serde_json::Value;

use crate::common::text;
use crate::model::{Scripted, client, effort, tool_model};
use crate::world::World;

/// v4's `MAX_TOOL_ROUNDS`, set through the admin tool-round setting.
pub const V4_TOOL_ROUNDS: u8 = 4;

/// Records posted cards; `fail` makes every post fail.
#[derive(Default)]
pub struct Ports {
    pub posted: Mutex<Vec<ProposalCard>>,
    pub refreshed: Mutex<Vec<String>>,
    pub pending: Vec<PendingCard>,
    pub fail: bool,
    /// Posting never completes (a stuck gateway).
    pub hang: bool,
}

impl ChatPorts for Ports {
    async fn refresh_proposals(&self, proposal_ids: &[String]) {
        self.refreshed
            .lock()
            .expect("refreshed")
            .extend_from_slice(proposal_ids);
    }

    async fn pending(&self) -> Vec<PendingCard> {
        self.pending.clone()
    }

    async fn post_card(&self, card: &ProposalCard) -> Result<(), String> {
        if self.hang {
            std::future::pending::<()>().await;
        }
        if self.fail {
            return Err("channel refused the card".into());
        }
        self.posted.lock().expect("posted").push(card.clone());
        Ok(())
    }
}

/// The world fixture's chat settings.
pub fn settings(input: &Value, tool_rounds: u8) -> AnswerSettings {
    let raw = &input["settings"];
    AnswerSettings {
        tool_rounds,
        timeout: Duration::from_secs_f64(raw["chat_pilot_timeout"].as_f64().expect("timeout")),
        reasoning: Some(effort(text(&raw["chat_pilot_think"]))),
        temperature: raw["chat_pilot_temperature"].as_f64(),
        max_output_tokens: u32::try_from(COMPLETION_RESERVE_TOKENS).expect("u32"),
        model_context_tokens: usize::try_from(
            raw["model_context_tokens"].as_u64().expect("context"),
        )
        .expect("usize"),
        clean_retry: true,
    }
}

/// One question in v4's full-set tool mode with plain settings (4 tool
/// rounds, a 1024-token reply, no reasoning or clean retry): `conversation`
/// from its system prompt on and the `model`'s scripted `actions`. Asserts
/// the answer did not fail; the provider comes back for its requests.
pub async fn ask_full_set(
    world: &mut World,
    ctx: &ToolContext,
    model: &str,
    conversation: Vec<Message>,
    actions: Vec<FakeAction>,
    window: usize,
) -> (Generation, Ports, Arc<Scripted>) {
    let provider = tool_model(actions);
    let (_governor, client) = client(Some(model), Arc::clone(&provider));
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let ports = Ports::default();
    let (guild, mut proposer) = world.question_parts();
    let question = Question {
        ctx,
        conversation,
        profanity: None,
        reminder: "Keep the tool facts exact.".into(),
        offer: ToolOffer::full_set(false),
        settings: AnswerSettings {
            tool_rounds: 4,
            timeout: Duration::from_secs(60),
            reasoning: None,
            temperature: None,
            max_output_tokens: 1024,
            model_context_tokens: window,
            clean_retry: false,
        },
    };
    let generation = answer(&deps, question, &guild, &mut proposer, &ports).await;
    assert_eq!(generation.failure, None, "{:?}", generation.failure);
    (generation, ports, provider)
}

/// The placeholder system prompt, then the member's `line`.
pub fn conversation(line: String) -> Vec<Message> {
    vec![
        Message::System {
            content: "SYSTEM".into(),
        },
        Message::User { content: line },
    ]
}

/// A plain answer in words.
pub fn said(model: &str, content: &str) -> CompletionResponse {
    CompletionResponse {
        reasoning_content: None,
        reasoning_tokens: None,
        model: model.to_owned(),
        content: Some(content.to_owned()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}
