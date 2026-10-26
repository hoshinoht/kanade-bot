//! The per-request budget: prior history is trimmed first, then old tool
//! result content is elided, until the whole request plus the completion
//! reserve fits the model context.

use serde::{Serialize, Serializer};
use serde_json::{Value, json};

use super::runs;
use kanade_extract::prompt::{TokenCounts as Counts, estimate_tokens};
use kanade_llm::Message;

/// The protected current turn alone does not fit the model context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "chat request estimate {estimate} exceeds context budget {budget} with completion reserve {reserve}"
)]
pub struct ContextBudgetError {
    /// The whole request's estimate, completion reserve included.
    pub estimate: usize,
    /// The route's resolved context window.
    pub budget: usize,
    /// The route's resolved completion reserve.
    pub reserve: usize,
}

/// Keeps an older assistant tool call paired with its tool response after the
/// response has been removed from the usable context.
pub const ELIDED_TOOL_RESULT: &str = "[tool result elided for context budget]";

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
/// the work it most recently requested. Returns the elided message's index.
fn elide_oldest_tool_result(messages: &mut [Message], current_user: usize) -> Option<usize> {
    let latest_round = messages
        .iter()
        .rposition(|message| matches!(message, Message::Assistant { tool_calls, .. } if !tool_calls.is_empty()))?;
    let from = current_user.saturating_add(1);
    let at = from + messages[from..latest_round].iter().position(
        |message| matches!(message, Message::Tool { content, .. } if content != ELIDED_TOOL_RESULT),
    )?;
    if let Message::Tool { content, .. } = &mut messages[at] {
        content.clear();
        content.push_str(ELIDED_TOOL_RESULT);
    }
    Some(at)
}

/// A request that fits: the messages to send and their estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct Budgeted {
    pub messages: Vec<Message>,
    /// Estimated prompt tokens (messages, tool calls and tool schemas),
    /// without the completion reserve.
    pub estimate: usize,
}

/// `messages` then `reminder` as one JSON array, without copying them.
struct Outgoing<'a>(&'a [Message], &'a Message);

impl Serialize for Outgoing<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter().chain([self.1]))
    }
}

/// What the runner reserves for `outgoing` beside `schemas`, before the
/// completion: the encoded request's bytes / 4 (multi-byte text such as CJK
/// costs it more than [`estimate_tokens`] says).
fn runner_tokens(outgoing: &Outgoing<'_>, schemas: &str) -> usize {
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
    // The request text is every message and the reminder, then the tool
    // calls, joined by blank lines; its counts are kept per piece and summed
    // so a trim step recounts only what it changed.
    let reminder_counts = Counts::of(reminder.text());
    let mut counts: Vec<Counts> = messages.iter().map(|m| Counts::of(m.text())).collect();
    let mut suffix_counts = Counts::of(&tool_suffix(messages));
    loop {
        let request = counts
            .iter()
            .fold(reminder_counts + suffix_counts, |sum, &each| sum + each)
            .with_seams(messages.len() + 1)
            .tokens();
        let estimate = request + schema_tokens;
        let total = estimate + completion_reserve;
        let has_block = matches!(
            messages.first(),
            Some(Message::System { content }) if runs::has_block(content, run_context)
        );
        let encoded = Outgoing(messages.as_slice(), &reminder);
        let over_call = || {
            estimate.max(runner_tokens(&encoded, schemas)) + completion_reserve
                > runs::call_window(model_context_tokens)
        };
        if total <= model_context_tokens && !(has_block && over_call()) {
            let mut outgoing = Vec::with_capacity(messages.len() + 1);
            outgoing.extend_from_slice(messages);
            outgoing.push(reminder);
            return Ok(Budgeted {
                messages: outgoing,
                estimate,
            });
        }
        if let Some(Message::System { content }) = messages.first_mut()
            && runs::trim_block(content, run_context)
        {
            counts[0] = Counts::of(content);
            continue;
        }
        if current_user > 1 {
            let removed = messages.remove(1);
            counts.remove(1);
            if let Message::Assistant { tool_calls, .. } = &removed
                && !tool_calls.is_empty()
            {
                suffix_counts = Counts::of(&tool_suffix(messages));
            }
            current_user -= 1;
            continue;
        }
        if let Some(at) = elide_oldest_tool_result(messages, current_user) {
            counts[at] = Counts::of(ELIDED_TOOL_RESULT);
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
    use kanade_llm::ToolCallRequest;

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

    const VOICE: &str = "voice 12345";

    /// The estimate as the request text joined whole.
    fn joined_estimate(messages: &[Message]) -> usize {
        let mut material: Vec<&str> = messages.iter().map(Message::text).collect();
        material.push(VOICE);
        estimate_tokens(&format!(
            "{}\n\n{}",
            material.join("\n\n"),
            tool_suffix(messages)
        )) + estimate_tokens("[]")
    }

    /// Ids, multi-byte text, empty assistant turns and a tool round in prior
    /// history, whose calls leave the tool-call suffix when it is dropped.
    fn mixed() -> Vec<Message> {
        let mut messages = vec![Message::System {
            content: "system: Lotus 123456789012345678 ✨".into(),
        }];
        messages.extend(round("call_100000001", "hist 9876543210 カンナ".into()));
        messages.push(user("Alvin <@123456789012345678>: and now?"));
        messages.extend(round("call_200000002", "Lotus 1234567 at 20:30".into()));
        messages.extend(round("call_300000003", "Will 55555 at 21:00".into()));
        messages
    }

    #[test]
    fn counts_add_over_blank_line_joins() {
        let texts = [
            "",
            "abc",
            "123456789012345678",
            "a 1234 b 12345 c ✨カンナ 99999999x",
            ELIDED_TOOL_RESULT,
        ];
        let summed = texts
            .iter()
            .map(|text| Counts::of(text))
            .fold(Counts::default(), |sum, counts| sum + counts)
            .with_seams(texts.len() - 1);
        assert_eq!(summed, Counts::of(&texts.join("\n\n")));
    }

    #[test]
    fn the_joined_estimate_is_the_fit_boundary() {
        let reserve = 50;
        let full = joined_estimate(&mixed());
        // Summing per-message estimates would truncate each piece and move
        // the boundary.
        let suffix = tool_suffix(&mixed());
        let pieces = mixed();
        let pieces = pieces
            .iter()
            .map(Message::text)
            .chain([VOICE, suffix.as_str(), "[]"]);
        assert!(pieces.map(estimate_tokens).sum::<usize>() < full);

        let mut messages = mixed();
        let fits = budgeted(&mut messages, "[]", VOICE, full + reserve, reserve, "").expect("fits");
        assert_eq!(fits.estimate, full);
        assert!(messages == mixed(), "nothing trimmed at the boundary");

        let mut messages = mixed();
        let fits =
            budgeted(&mut messages, "[]", VOICE, full + reserve - 1, reserve, "").expect("fits");
        assert_eq!(
            messages.len(),
            mixed().len() - 1,
            "one history turn dropped"
        );
        assert_eq!(fits.estimate, joined_estimate(&messages));
    }

    #[test]
    fn every_trim_step_estimates_the_joined_request() {
        let reserve = 50;
        for window in 0..=joined_estimate(&mixed()) + reserve {
            let mut messages = mixed();
            match budgeted(&mut messages, "[]", VOICE, window, reserve, "") {
                Ok(fits) => {
                    assert_eq!(fits.estimate, joined_estimate(&messages), "window {window}");
                    assert!(fits.estimate + reserve <= window);
                    assert!(fits.messages[..messages.len()] == messages[..]);
                    assert_eq!(fits.messages.last().map(Message::text), Some(VOICE));
                }
                Err(error) => {
                    assert_eq!(error.estimate, joined_estimate(&messages) + reserve);
                    assert!(error.estimate > window);
                }
            }
        }
    }
}
