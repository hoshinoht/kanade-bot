//! A question as one `chat_interactions` row with its rounds (D1), with the
//! Chat page's outcome (`docs/v5/admin-api.md`).

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::{AnswerFailure, Generation};
use crate::chat::sanitize::looks_like_clarification;
use crate::chat::tools::{REFUSED, ToolContext, ToolName};
use crate::domain::model_log::{ChatInteraction, ChatOutcome, ChatRound};
use crate::infrastructure::llm::governor::{Refused, SessionFailure};
use crate::infrastructure::llm::{Effort, ErrorCode};

/// How the question ended. `rate_limited` and `withheld` are decided before
/// the loop runs (gate, pre-screen) and are the caller's to record.
fn chat_guardrail(generation: &Generation) -> Value {
    let mut guardrail = serde_json::Map::new();
    if generation.is_blocked() || generation.failure == Some(AnswerFailure::ContentBlocked) {
        guardrail.insert("content_filter".into(), Value::Bool(true));
    }
    if generation.external_unmasked {
        guardrail.insert("external_unmasked".into(), Value::Bool(true));
    }
    Value::Object(guardrail)
}

pub fn chat_outcome(generation: &Generation) -> ChatOutcome {
    if let Some(failure) = &generation.failure {
        return match failure {
            AnswerFailure::ContentBlocked => ChatOutcome::ContentBlocked,
            AnswerFailure::Timeout { .. } => ChatOutcome::Timeout,
            AnswerFailure::Session(error) => match &error.failure {
                SessionFailure::Refused(
                    Refused::Busy
                    | Refused::Timeout
                    | Refused::Unavailable { .. }
                    | Refused::RateLimited { .. },
                ) => ChatOutcome::TurnedAway,
                SessionFailure::Model(model)
                    if matches!(
                        model.code,
                        ErrorCode::AdmissionRefused | ErrorCode::BackendUnavailable
                    ) =>
                {
                    ChatOutcome::TurnedAway
                }
                _ => ChatOutcome::Error,
            },
            _ => ChatOutcome::Error,
        };
    }
    let refused = generation.outcomes.iter().any(|o| {
        o.outcome.error == Some(REFUSED)
            && ToolName::parse(&o.outcome.name).is_some_and(ToolName::is_write)
    });
    if !refused {
        ChatOutcome::Answered
    } else if looks_like_clarification(&generation.reply) {
        ChatOutcome::Clarified
    } else {
        ChatOutcome::Refused
    }
}

/// Bytes of a tool result kept in the log.
const RESULT_CAP: usize = 8 * 1024;

/// The result as logged: at most [`RESULT_CAP`] bytes cut on a char
/// boundary, then a marker with the full length in bytes.
fn capped_result(output: &str) -> String {
    if output.len() <= RESULT_CAP {
        return output.to_owned();
    }
    let mut end = RESULT_CAP;
    while !output.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [truncated, {} bytes]", &output[..end], output.len())
}

fn round_calls(generation: &Generation, round: u32, clean: bool) -> Value {
    if clean {
        return json!([]);
    }
    json!(
        generation
            .outcomes
            .iter()
            .filter(|o| o.round == round)
            .map(|o| json!({
                "name": o.outcome.name,
                "outcome": o.outcome.outcome(),
                "arguments": o.outcome.arguments,
                "created": o.outcome.created,
                "posted": o.posted,
                // Pre-encoding text: real names, not the model's identity tokens.
                "result": capped_result(&o.outcome.output),
                "took_ms": o.took_ms,
            }))
            .collect::<Vec<_>>()
    )
}

/// The log row: never the prompt, only the question, reply and rounds.
#[allow(clippy::too_many_arguments)]
pub fn interaction(
    id: String,
    at: DateTime<Utc>,
    ctx: &ToolContext,
    question: &str,
    generation: &Generation,
    model: &str,
    reasoning: Option<Effort>,
    latency_ms: u64,
) -> ChatInteraction {
    let rounds = generation
        .model_rounds
        .iter()
        .map(|round| ChatRound {
            model: model.to_owned(),
            reasoning: reasoning.map(|effort| effort.as_str().to_owned()),
            finish_reason: round.finish_reason.clone(),
            latency_ms: Some(round.latency_ms),
            tool_bundles: round.bundles.clone(),
            tools: round.requested_tools.clone(),
            tool_calls: round_calls(generation, round.round, round.clean),
            response: round.content.clone(),
        })
        .collect();
    ChatInteraction {
        id,
        at,
        channel_id: Some(ctx.channel_id.clone()),
        message_id: Some(ctx.message_id.clone()),
        member_id: Some(ctx.author_id.clone()),
        question: question.to_owned(),
        reply: generation.reply.clone(),
        outcome: chat_outcome(generation),
        error: generation.failure.as_ref().map(ToString::to_string),
        clean_retry: generation.clean_retry,
        withheld: false,
        guardrail: chat_guardrail(generation),
        request_count: generation.requests,
        latency_ms: Some(latency_ms),
        model_ms: Some(generation.model_ms),
        tools_ms: Some(generation.tools_ms),
        prompt_tokens: generation.prompt_tokens,
        completion_tokens: generation.completion_tokens,
        rounds,
    }
}
