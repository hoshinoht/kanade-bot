use std::ops::Add;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::extract::text::pattern;
use crate::infrastructure::llm::{Message, schema_instruction};

/// Characters per token for this prompt: dense timestamps, boss names and
/// punctuation measure nearer 3 than prose's 4 against `gpt-oss:20b`.
pub const CHARS_PER_TOKEN: f64 = 2.8;

/// An 18-digit id tokenises at about one token per two digits, so ids cost
/// about 4x their length suggests.
pub const TOKENS_PER_ID: usize = 8;

/// Tokens held back from the context for the answer: a prompt that leaves no
/// room comes back truncated (`finish_reason="length"`) and fails to validate.
pub const CONTEXT_RESERVE: usize = 2500;

/// Digit runs this long are ids, not times or levels.
static ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"\d{5,}"));

/// What [`estimate_tokens`] counts in a text: its ids and the characters left
/// beside them. Unlike the estimate, which truncates, the counts add over
/// texts joined by a non-digit separator, since no id spans the seam.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenCounts {
    ids: usize,
    rest: usize,
}

impl TokenCounts {
    pub fn of(text: &str) -> Self {
        let (ids, id_chars) = ID.find_iter(text).fold((0, 0), |(ids, chars), found| {
            (ids + 1, chars + found.as_str().chars().count())
        });
        Self {
            ids,
            rest: text.chars().count() - id_chars,
        }
    }

    /// `separators` blank-line (`"\n\n"`) seams on top.
    #[must_use]
    pub fn with_seams(self, separators: usize) -> Self {
        Self {
            rest: self.rest + 2 * separators,
            ..self
        }
    }

    /// [`estimate_tokens`] of the counted text.
    pub fn tokens(self) -> usize {
        // Truncation, as v4's `int()` of a non-negative float.
        self.ids * TOKENS_PER_ID + (self.rest as f64 / CHARS_PER_TOKEN) as usize
    }
}

impl Add for TokenCounts {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            ids: self.ids + other.ids,
            rest: self.rest + other.rest,
        }
    }
}

/// Roughly what `text` costs the model, erring high: it decides whether a
/// burst is split, so guessing low is the expensive mistake.
pub fn estimate_tokens(text: &str) -> usize {
    TokenCounts::of(text).tokens()
}

/// Every message's content joined by blank lines, as v4 logs the prompt.
pub fn prompt_text(messages: &[Message]) -> String {
    let contents: Vec<&str> = messages.iter().map(Message::text).collect();
    contents.join("\n\n")
}

/// [`estimate_tokens`] over a whole `messages` payload.
pub fn estimate_messages(messages: &[Message]) -> usize {
    estimate_tokens(&prompt_text(messages))
}

/// The most a prompt may cost and still leave the model room to answer.
pub fn prompt_budget(num_ctx: usize) -> usize {
    num_ctx.saturating_sub(CONTEXT_RESERVE).max(CONTEXT_RESERVE)
}

/// Context budget with a role-resolved completion reserve.
pub fn prompt_budget_with_reserve(num_ctx: usize, reserve: usize) -> usize {
    num_ctx.saturating_sub(reserve)
}

/// What the runner's schema-in-prompt instruction adds to an estimated prompt
/// for a model without structured output (merged after a blank line).
pub fn schema_instruction_tokens(schema: &Value) -> usize {
    let instruction = schema_instruction(schema).expect("a JSON value serializes");
    estimate_tokens(&format!("\n\n{instruction}"))
}
