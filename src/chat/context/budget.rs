//! The per-request budget (v4 `_budgeted_messages`): only prior history is
//! trimmed, oldest first, until the whole request plus the completion reserve
//! fits the model context.

use std::fmt;

use serde_json::{Value, json};

use super::COMPLETION_RESERVE_TOKENS;
use crate::extract::prompt::estimate_tokens;
use crate::infrastructure::llm::Message;

/// The protected current turn alone does not fit the model context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBudgetError {
    pub estimate: usize,
    pub budget: usize,
}

impl fmt::Display for ContextBudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "chat request estimate {} exceeds context budget {} with completion reserve",
            self.estimate, self.budget
        )
    }
}

impl std::error::Error for ContextBudgetError {}

fn content(message: &Message) -> &str {
    match message {
        Message::System { content } | Message::User { content } | Message::Tool { content, .. } => {
            content
        }
        Message::Assistant { content, .. } => content.as_deref().unwrap_or_default(),
    }
}

/// Every assistant turn's tool calls as v4 serialised them for the estimate.
fn tool_suffix(messages: &[Message]) -> String {
    let calls: Vec<Value> = messages
        .iter()
        .filter_map(|message| match message {
            Message::Assistant { tool_calls, .. } if !tool_calls.is_empty() => Some(Value::Array(
                tool_calls
                    .iter()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {"name": call.name, "arguments": call.arguments},
                        })
                    })
                    .collect(),
            )),
            _ => None,
        })
        .collect();
    Value::Array(calls).to_string()
}

/// The request to send: `messages` plus the voice `reminder`, after dropping
/// prior history from `messages` itself (the trim sticks for later rounds).
/// `schemas` is the offered tools' compact JSON (`[]` for none).
///
/// # Errors
/// [`ContextBudgetError`] when nothing but the current turn is left and it
/// still does not fit.
pub fn budgeted(
    messages: &mut Vec<Message>,
    schemas: &str,
    reminder: &str,
    model_context_tokens: usize,
) -> Result<Vec<Message>, ContextBudgetError> {
    let mut current_user = messages
        .iter()
        .rposition(|message| matches!(message, Message::User { .. }))
        .unwrap_or(1);
    let schema_tokens = estimate_tokens(schemas);
    let suffix = tool_suffix(messages);
    let reminder = Message::User {
        content: reminder.to_owned(),
    };
    loop {
        let mut outgoing = messages.clone();
        outgoing.push(reminder.clone());
        let material: Vec<&str> = outgoing.iter().map(content).collect();
        let request = estimate_tokens(&format!("{}\n\n{suffix}", material.join("\n\n")));
        let total = request + schema_tokens + COMPLETION_RESERVE_TOKENS;
        if total <= model_context_tokens {
            return Ok(outgoing);
        }
        if current_user <= 1 {
            return Err(ContextBudgetError {
                estimate: total,
                budget: model_context_tokens,
            });
        }
        messages.remove(1);
        current_user -= 1;
    }
}
