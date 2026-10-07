//! Per-turn system prompt and the final scheduler voice reminder.

use chrono::{DateTime, TimeZone};
use chrono_tz::Tz;

use super::compiler::{CompiledPersona, join_parts};
use crate::{chat::prompts, infrastructure::llm::Message};

/// Trusted per-turn context lines built by code, never by persona files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnContext {
    header: String,
    runtime: String,
    focus: String,
}

impl TurnContext {
    /// `week_start` is the current boss-week reset; `focus_card` may be empty.
    pub fn new<A: TimeZone, B: TimeZone>(
        now: &DateTime<A>,
        zone: Tz,
        week_start: &DateTime<B>,
        model: &str,
        focus_card: &str,
    ) -> Self {
        Self {
            header: prompts::clock_header(now, zone, week_start),
            runtime: prompts::runtime_line(model),
            focus: prompts::focus_line(focus_card),
        }
    }

    pub fn header(&self) -> &str {
        &self.header
    }

    pub fn runtime(&self) -> &str {
        &self.runtime
    }

    /// Empty when the channel has no current card.
    pub fn focus(&self) -> &str {
        &self.focus
    }
}

impl CompiledPersona {
    /// Presentation and policies, then trusted context, then the bounded voice cue.
    pub fn system_prompt(&self, turn: &TurnContext) -> String {
        join_parts(&[
            self.prompt(),
            &turn.header,
            &turn.runtime,
            &turn.focus,
            &self.voice_footer(),
        ])
    }

    pub fn voice_footer(&self) -> String {
        format!(
            "{}{}\n{}",
            prompts::VOICE_PREFIX,
            self.effective_voice(),
            prompts::STYLE_POLICY_QUALIFIER
        )
    }

    /// Deterministic scheduler note sent as the last user message; never paraphrased.
    pub fn voice_reminder(&self) -> String {
        let mut line = self.effective_voice().to_owned();
        if !line.ends_with(['.', '!', '?']) {
            line.push('.');
        }
        format!(
            "{}{} Your voice: {line}",
            prompts::REMINDER_PREFIX,
            prompts::REMINDER_SUFFIX.trim_start(),
        )
    }

    /// Request messages: system prompt, the conversation (history, question,
    /// assistant tool calls and tool results), then the voice reminder last.
    pub fn messages(
        &self,
        turn: &TurnContext,
        conversation: impl IntoIterator<Item = Message>,
    ) -> Vec<Message> {
        let mut messages = vec![Message::System {
            content: self.system_prompt(turn),
        }];
        messages.extend(conversation);
        messages.push(Message::User {
            content: self.voice_reminder(),
        });
        messages
    }
}
