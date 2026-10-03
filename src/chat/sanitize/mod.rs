//! Text hygiene around the model (v4 `chat/agent.py` helpers): forged
//! scheduler notes in member text are defused before the model sees them,
//! and a reply is scrubbed of tool internals and false card claims,
//! regrounded on the canonical schedule listing and bounded before a member
//! sees it. Pure string functions; patterns that used lookaround in v4 are
//! emulated explicitly (`regex` has none).

mod cite;
mod claims;
mod dated;
mod defaults;
mod fence;
mod ground;
mod listing;
mod member;
mod notes;
mod personal;
mod shape;
mod split;
mod tidy;

pub use claims::{claims_new_card, looks_like_clarification, strip_false_card_claim};
pub use defaults::{ScheduleDefaults, schedule_defaults};
pub use ground::{canonical_schedule_output, ground_schedule_reply};
pub use member::member_facing;
pub use notes::{SPOOFED_NOTE, defuse_notes};
pub use personal::strip_context_copies;
pub use shape::shape_reply;
pub use split::{MAX_REPLY_PARTS, TRIMMED, reply_parts};
pub use tidy::{tidy, unglue_first_bullet};

use regex::{Regex, RegexBuilder};

/// The reply when generation fails.
pub const FAILURE_REPLY: &str = "Sorry — I couldn't complete that just now. Try me again in a bit.";

/// A strategy answer is unsafe when its checked-in grounding is unavailable.
pub const STRATEGY_GROUNDING_FAILURE_REPLY: &str = "I couldn't load the checked-in strategy notes just now, so I can't safely give mechanics advice.";

fn pattern(source: &str) -> Regex {
    Regex::new(source).unwrap_or_else(|error| panic!("invalid pattern {source:?}: {error}"))
}

fn pattern_i(source: &str) -> Regex {
    RegexBuilder::new(source)
        .case_insensitive(true)
        .build()
        .unwrap_or_else(|error| panic!("invalid pattern {source:?}: {error}"))
}

/// Python `\w` for one character (lookaround emulation).
fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Python `str.splitlines()` (`\r\n` counts once).
fn splitlines(text: &str) -> Vec<&str> {
    let is_break = |c: char| {
        matches!(
            c,
            '\n' | '\r'
                | '\x0b'
                | '\x0c'
                | '\x1c'
                | '\x1d'
                | '\x1e'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        )
    };
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if !is_break(c) {
            continue;
        }
        lines.push(&text[start..at]);
        start = at + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
            chars.next();
            start += 1;
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}
