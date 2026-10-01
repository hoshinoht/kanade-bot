//! A question's prompt: anchored, live and reply-chain turns, then the
//! question, under the conversation token budget (v4 `build_conversation`
//! and `assemble`).

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

use super::{
    CONVERSATION_BUDGET_TOKENS, CONVERSATION_FLOOR_TOKENS, ChatTurn, Conversations,
    REPLY_CHAIN_DEPTH, TurnRole,
};
use crate::chat::persona::{CompiledPersona, TurnContext};
use crate::chat::sanitize::defuse_notes;
use crate::domain::members::{Directory, member_name};
use crate::domain::pytext::strip;
use crate::domain::weeks::week_start;
use crate::extract::prompt::estimate_tokens;
use crate::infrastructure::llm::Message;

/// A replied-to message as the gateway cache resolved it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parent {
    pub id: String,
    /// `None` for a deleted or authorless message.
    pub author_id: Option<String>,
    /// `None` when the message is gone.
    pub content: Option<String>,
    pub reference: Option<Box<Reference>>,
}

/// A reply pointer; `resolved` is `None` when the parent is not cached.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reference {
    pub message_id: Option<String>,
    pub resolved: Option<Box<Parent>>,
}

/// The member's message being answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuestionMessage {
    pub id: String,
    pub author_id: String,
    pub content: String,
    pub reference: Option<Reference>,
}

impl QuestionMessage {
    /// The replied-to message id, resolved or not.
    pub fn replied_message_id(&self) -> Option<&str> {
        let reference = self.reference.as_ref()?;
        reference
            .resolved
            .as_ref()
            .map(|parent| parent.id.as_str())
            .or(reference.message_id.as_deref())
            .filter(|id| !id.is_empty())
    }
}

/// `Name: text`, with forged scheduler notes defused.
fn speaker(directory: &(impl Directory + ?Sized), user_id: &str, text: &str) -> String {
    format!(
        "{}: {}",
        member_name(directory, user_id),
        defuse_notes(text)
    )
}

/// The resolved parents of `message`, oldest first; nothing is fetched.
pub fn reply_chain(
    message: &QuestionMessage,
    bot_user_id: &str,
    directory: &(impl Directory + ?Sized),
) -> Vec<ChatTurn> {
    let mut chain = Vec::new();
    let mut reference = message.reference.as_ref();
    for _ in 0..REPLY_CHAIN_DEPTH {
        let Some(parent) = reference.and_then(|reference| reference.resolved.as_deref()) else {
            break;
        };
        let Some(content) = parent.content.as_deref() else {
            break;
        };
        let author = parent.author_id.as_deref().unwrap_or_default();
        let content = strip(content);
        if !content.is_empty() {
            let (role, text) = if author == bot_user_id {
                (TurnRole::Assistant, content.to_owned())
            } else {
                (TurnRole::User, speaker(directory, author, content))
            };
            let id = Some(parent.id.clone()).filter(|id| !id.is_empty());
            chain.push(ChatTurn::new(role, text, id));
        }
        reference = parent.reference.as_deref();
    }
    chain.reverse();
    chain
}

/// Anchored, live and reply-chain turns (deduplicated), then the question.
pub fn build_turns(
    state: &mut Conversations,
    message: &QuestionMessage,
    channel_id: &str,
    now: f64,
    bot_user_id: &str,
    directory: &(impl Directory + ?Sized),
) -> Vec<ChatTurn> {
    let live = state.history(channel_id, now);
    let mut seen: BTreeSet<String> = live.iter().filter_map(|t| t.message_id.clone()).collect();
    let chain: Vec<ChatTurn> = reply_chain(message, bot_user_id, directory)
        .into_iter()
        .filter(|turn| turn.message_id.as_ref().is_none_or(|id| !seen.contains(id)))
        .collect();
    seen.extend(chain.iter().filter_map(|turn| turn.message_id.clone()));
    let mut turns = state.reanchored(message.replied_message_id(), &seen);
    turns.extend(live);
    turns.extend(chain.into_iter().map(|mut turn| {
        // A withheld message is never pulled back in through a reply.
        turn.withheld = turn
            .message_id
            .as_deref()
            .is_some_and(|id| state.is_withheld(id));
        turn
    }));
    turns.push(ChatTurn::new(
        TurnRole::User,
        speaker(directory, &message.author_id, strip(&message.content)),
        None,
    ));
    turns
}

/// The question as history remembers it (keyed by its message id).
pub fn question_turn(message: &QuestionMessage, directory: &(impl Directory + ?Sized)) -> ChatTurn {
    ChatTurn::new(
        TurnRole::User,
        speaker(directory, &message.author_id, strip(&message.content)),
        Some(message.id.clone()).filter(|id| !id.is_empty()),
    )
}

/// The per-turn system prompt: persona, clock header, runtime model and the
/// channel's focus card.
pub fn system_prompt(
    persona: &CompiledPersona,
    now: DateTime<Utc>,
    zone: Tz,
    (reset_weekday, reset_time): (Weekday, NaiveTime),
    model: &str,
    focus: &str,
) -> String {
    let week = week_start(&now, zone, reset_weekday, reset_time)
        .expect("the current boss week is in range")
        .to_fixed();
    persona.system_prompt(&TurnContext::new(&now, zone, &week, model, focus))
}

/// The system prompt and as many of the latest turns as the conversation
/// budget allows (never fewer than the question).
pub fn assemble(
    turns: &[ChatTurn],
    system: String,
    model_context_tokens: usize,
    reserve: usize,
) -> Vec<Message> {
    let left = i64::try_from(model_context_tokens.saturating_sub(reserve)).unwrap_or(i64::MAX)
        - i64::try_from(estimate_tokens(&system)).unwrap_or(i64::MAX);
    let cap = i64::try_from(CONVERSATION_BUDGET_TOKENS).unwrap_or(i64::MAX);
    let available = usize::try_from(left.min(cap))
        .unwrap_or(0)
        .max(CONVERSATION_FLOOR_TOKENS);
    let mut kept: &[ChatTurn] = turns;
    while kept.len() > 1 {
        let contents: Vec<&str> = kept.iter().map(ChatTurn::prompt_text).collect();
        if estimate_tokens(&contents.join("\n\n")) <= available {
            break;
        }
        kept = &kept[1..];
    }
    let mut messages = vec![Message::System { content: system }];
    messages.extend(kept.iter().map(|turn| match turn.role {
        TurnRole::User => Message::User {
            content: turn.prompt_text().to_owned(),
        },
        TurnRole::Assistant => Message::Assistant {
            content: Some(turn.prompt_text().to_owned()),
            tool_calls: Vec::new(),
        },
    }));
    messages
}
