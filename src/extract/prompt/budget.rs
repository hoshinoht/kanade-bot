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

/// Roughly what `text` costs the model, erring high: it decides whether a
/// burst is split, so guessing low is the expensive mistake.
pub fn estimate_tokens(text: &str) -> usize {
    let ids = ID.find_iter(text).count();
    let rest = ID.replace_all(text, "").chars().count();
    // Truncation, as v4's `int()` of a non-negative float.
    ids * TOKENS_PER_ID + (rest as f64 / CHARS_PER_TOKEN) as usize
}

fn content(message: &Message) -> &str {
    match message {
        Message::System { content } | Message::User { content } | Message::Tool { content, .. } => {
            content
        }
        Message::Assistant { content, .. } => content.as_deref().unwrap_or_default(),
    }
}

/// Every message's content joined by blank lines, as v4 logs the prompt.
pub fn prompt_text(messages: &[Message]) -> String {
    let contents: Vec<&str> = messages.iter().map(content).collect();
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

/// What the runner's schema-in-prompt instruction adds to an estimated prompt
/// for a model without structured output (merged after a blank line).
pub fn schema_instruction_tokens(schema: &Value) -> usize {
    let instruction = schema_instruction(schema).expect("a JSON value serializes");
    estimate_tokens(&format!("\n\n{instruction}"))
}
