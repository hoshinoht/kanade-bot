//! The per-request budget: prior history is trimmed first, then old tool
//! result content is elided, until the whole request plus the completion
//! reserve fits the model context.

use std::fmt;

use serde_json::{Value, json};

use super::runs;
use crate::extract::prompt::estimate_tokens;
use crate::infrastructure::llm::Message;

/// The protected current turn alone does not fit the model context.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBudgetError {
    /// The whole request's estimate, completion reserve included.
    pub estimate: usize,
    /// The route's resolved context window.
    pub budget: usize,
    /// The route's resolved completion reserve.
    pub reserve: usize,
}

impl fmt::Display for ContextBudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "chat request estimate {} exceeds context budget {} with completion reserve {}",
            self.estimate, self.budget, self.reserve
        )
    }
}

impl std::error::Error for ContextBudgetError {}

/// Keeps an older assistant tool call paired with its tool response after the
/// response has been removed from the usable context.
pub const ELIDED_TOOL_RESULT: &str = "[tool result elided for context budget]";

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

/// Elide one old result while keeping every assistant `tool_call` paired with
/// a tool message. The latest tool round stays intact so the model can finish
/// the work it most recently requested.
fn elide_oldest_tool_result(messages: &mut [Message], current_user: usize) -> bool {
    let Some(latest_round) = messages
        .iter()
        .rposition(|message| matches!(message, Message::Assistant { tool_calls, .. } if !tool_calls.is_empty()))
    else {
        return false;
    };
    messages[current_user.saturating_add(1)..latest_round]
        .iter_mut()
        .find_map(|message| match message {
            Message::Tool { content, .. } if content != ELIDED_TOOL_RESULT => Some(content),
            _ => None,
        })
        .map(|content| {
            content.clear();
            content.push_str(ELIDED_TOOL_RESULT);
        })
        .is_some()
}

/// A request that fits: the messages to send and their estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct Budgeted {
    pub messages: Vec<Message>,
    /// Estimated prompt tokens (messages, tool calls and tool schemas),
    /// without the completion reserve.
    pub estimate: usize,
}

/// What the runner reserves for `outgoing` beside `schemas`, before the
/// completion: the encoded request's bytes / 4 (multi-byte text such as CJK
/// costs it more than [`estimate_tokens`] says).
fn runner_tokens(outgoing: &[Message], schemas: &str) -> usize {
    serde_json::to_vec(outgoing)
        .map_or(usize::MAX, |bytes| bytes.len())
        .saturating_add(schemas.len())
        .div_ceil(4)
}

/// The request to send: `messages` plus the voice `reminder`, after trimming
/// the system prompt's copy of `run_context` (the question's `D-RUN-CONTEXT`
/// block, `""` for none), then dropping prior history and then eliding older
/// tool results from `messages` itself (the trim sticks for later rounds).
/// While a copy is there it also yields whenever the request, costed as the
/// higher of this estimate and the runner's bytes / 4, plus the reserve is
/// over one call's token budget (`CALL_TOKEN_BUDGET`): it may cost nothing
/// else, and never the call.
/// `schemas` is the offered tools' compact JSON (`[]` for none).
///
/// # Errors
/// [`ContextBudgetError`] when the protected current turn and latest tool
/// results still do not fit.
pub fn budgeted(
    messages: &mut Vec<Message>,
    schemas: &str,
    reminder: &str,
    model_context_tokens: usize,
    completion_reserve: usize,
    run_context: &str,
) -> Result<Budgeted, ContextBudgetError> {
    let mut current_user = messages
        .iter()
        .rposition(|message| matches!(message, Message::User { .. }))
        .unwrap_or(1);
    let schema_tokens = estimate_tokens(schemas);
    let reminder = Message::User {
        content: reminder.to_owned(),
    };
    loop {
        let mut outgoing = messages.clone();
        outgoing.push(reminder.clone());
        let material: Vec<&str> = outgoing.iter().map(content).collect();
        let request = estimate_tokens(&format!(
            "{}\n\n{}",
            material.join("\n\n"),
            tool_suffix(messages)
        ));
        let estimate = request + schema_tokens;
        let total = estimate + completion_reserve;
        let has_block = matches!(
            messages.first(),
            Some(Message::System { content }) if runs::has_block(content, run_context)
        );
        let over_call = || {
            estimate.max(runner_tokens(&outgoing, schemas)) + completion_reserve
                > runs::call_window(model_context_tokens)
        };
        if total <= model_context_tokens && !(has_block && over_call()) {
            return Ok(Budgeted {
                messages: outgoing,
                estimate,
            });
        }
        if let Some(Message::System { content }) = messages.first_mut()
            && runs::trim_block(content, run_context)
        {
            continue;
        }
        if current_user > 1 {
            messages.remove(1);
            current_user -= 1;
            continue;
        }
        if elide_oldest_tool_result(messages, current_user) {
            continue;
        }
        return Err(ContextBudgetError {
            estimate: total,
            budget: model_context_tokens,
            reserve: completion_reserve,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::llm::ToolCallRequest;

    /// About 1000 estimated tokens of letters (digits would count as ids).
    fn bulk(letter: char) -> String {
        std::iter::repeat_n(letter, 2800).collect()
    }

    fn user(content: impl Into<String>) -> Message {
        Message::User {
            content: content.into(),
        }
    }

    fn round(id: &str, result: String) -> [Message; 2] {
        [
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: id.into(),
                    name: "list_fixed".into(),
                    arguments: "{}".into(),
                }],
            },
            Message::Tool {
                tool_call_id: id.into(),
                content: result,
            },
        ]
    }

    /// System, two prior history turns, the question, then two tool rounds.
    fn conversation() -> Vec<Message> {
        let mut messages = vec![
            Message::System {
                content: "system".into(),
            },
            user(bulk('h')),
            Message::Assistant {
                content: Some(bulk('r')),
                tool_calls: Vec::new(),
            },
            user("Alvin: and now?"),
        ];
        messages.extend(round("a1", bulk('a')));
        messages.extend(round("b1", bulk('b')));
        messages
    }

    fn tool_result<'a>(messages: &'a [Message], id: &str) -> &'a str {
        messages
            .iter()
            .find_map(|message| match message {
                Message::Tool {
                    tool_call_id,
                    content,
                } if tool_call_id == id => Some(content.as_str()),
                _ => None,
            })
            .expect("tool result present")
    }

    /// Every assistant call is answered by exactly one later tool message.
    fn assert_paired(messages: &[Message]) {
        for (at, message) in messages.iter().enumerate() {
            if let Message::Assistant { tool_calls, .. } = message {
                for call in tool_calls {
                    let answers = messages[at + 1..]
                        .iter()
                        .filter(|m| matches!(m, Message::Tool { tool_call_id, .. } if *tool_call_id == call.id))
                        .count();
                    assert_eq!(answers, 1, "call {} unpaired", call.id);
                }
            }
        }
        let calls = messages
            .iter()
            .map(|m| match m {
                Message::Assistant { tool_calls, .. } => tool_calls.len(),
                _ => 0,
            })
            .sum::<usize>();
        let results = messages
            .iter()
            .filter(|m| matches!(m, Message::Tool { .. }))
            .count();
        assert_eq!(calls, results);
    }

    fn is_question(message: &Message) -> bool {
        matches!(message, Message::User { content } if content == "Alvin: and now?")
    }

    #[test]
    fn prior_history_goes_before_any_tool_result() {
        let mut messages = conversation();
        let outgoing = budgeted(&mut messages, "[]", "voice", 2900, 100, "")
            .expect("fits")
            .messages;
        assert_eq!(messages.len(), 6, "both history turns dropped");
        assert!(is_question(&messages[1]));
        assert_eq!(tool_result(&messages, "a1"), bulk('a'));
        assert_eq!(tool_result(&messages, "b1"), bulk('b'));
        assert_eq!(outgoing.len(), messages.len() + 1);
    }

    #[test]
    fn older_tool_results_are_elided_but_the_latest_round_stays() {
        let mut messages = conversation();
        let outgoing = budgeted(&mut messages, "[]", "voice", 2000, 100, "")
            .expect("fits")
            .messages;
        assert!(is_question(&messages[1]));
        assert_eq!(tool_result(&messages, "a1"), ELIDED_TOOL_RESULT);
        assert_eq!(tool_result(&messages, "b1"), bulk('b'));
        assert_paired(&messages);
        assert_paired(&outgoing);
    }

    #[test]
    fn the_resolved_reserve_decides_the_fit() {
        let lone = || {
            vec![
                Message::System {
                    content: "system".into(),
                },
                user(bulk('q')),
            ]
        };
        let bare = budgeted(&mut lone(), "[]", "voice", 0, 0, "")
            .unwrap_err()
            .estimate;
        // Well under the old 1024 constant, then well over it.
        for reserve in [10, 3000] {
            let window = bare + reserve;
            let fits = budgeted(&mut lone(), "[]", "voice", window, reserve, "").expect("fits");
            assert_eq!(fits.estimate, bare, "the estimate leaves out the reserve");
            let error = budgeted(&mut lone(), "[]", "voice", window - 1, reserve, "").unwrap_err();
            assert_eq!(
                error,
                ContextBudgetError {
                    estimate: window,
                    budget: window - 1,
                    reserve,
                }
            );
        }
    }

    #[test]
    fn only_the_protected_turn_left_is_a_budget_error() {
        let mut messages = conversation();
        let error = budgeted(&mut messages, "[]", "voice", 500, 100, "").unwrap_err();
        assert_eq!((error.budget, error.reserve), (500, 100));
        assert!(error.estimate > 500);
        assert_eq!(
            error.to_string(),
            format!(
                "chat request estimate {} exceeds context budget 500 with completion reserve 100",
                error.estimate
            )
        );
        // Everything trimmable was trimmed; the protected material stays.
        assert!(is_question(&messages[1]));
        assert_eq!(tool_result(&messages, "a1"), ELIDED_TOOL_RESULT);
        assert_eq!(tool_result(&messages, "b1"), bulk('b'));
        assert_paired(&messages);
    }
}
