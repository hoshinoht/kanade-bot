//! What of a chat prompt is code-owned: the pieces encoding must leave
//! literal and the words the boundary scanner may therefore meet unmasked.

use super::context::{
    CLOCK_HEADER_END, CLOCK_HEADER_START, FOCUS_PREFIX, FOCUS_SUFFIX, MONTHS, RUNTIME_LINE_END,
    RUNTIME_LINE_START, WEEKDAYS, clock_header, focus_line, runtime_line,
};
use super::{
    ASSISTANT_NAME_FIELD, ASSISTANT_SCOPE, BOSS_KNOWLEDGE_POLICY, DEFAULT_VOICE, EXAMPLES_HEADING,
    GROUNDING_POLICY, REMINDER_PREFIX, REMINDER_SUFFIX, SCHEDULER_POLICY, STYLE_POLICY_QUALIFIER,
    VOICE_PREFIX,
};
use crate::chat::persona::py_strip;
use crate::infrastructure::llm::identity::Protected;

/// Fixed texts: policies (as the persona compiler strips them), the scope
/// around the assistant name, voice and reminder cues.
fn fixed() -> Vec<String> {
    let mut texts: Vec<String> = py_strip(ASSISTANT_SCOPE)
        .split(ASSISTANT_NAME_FIELD)
        .map(str::to_owned)
        .collect();
    texts.extend(
        [
            py_strip(SCHEDULER_POLICY),
            py_strip(GROUNDING_POLICY),
            py_strip(BOSS_KNOWLEDGE_POLICY),
            VOICE_PREFIX,
            STYLE_POLICY_QUALIFIER,
            DEFAULT_VOICE,
            EXAMPLES_HEADING,
            REMINDER_PREFIX,
            REMINDER_SUFFIX,
            FOCUS_PREFIX,
            FOCUS_SUFFIX,
        ]
        .map(str::to_owned),
    );
    texts.retain(|text| !text.trim().is_empty());
    texts
}

/// The code-owned pieces of the system prompt and voice reminder: copied
/// verbatim by `encode_protected`, so a member named like a rules word
/// (`Will`) never rewrites the rules. Persona text, the focus card and the
/// conversation are encoded.
pub fn protected() -> Vec<Protected> {
    let mut pieces: Vec<Protected> = fixed().into_iter().map(Protected::Exact).collect();
    pieces.push(Protected::Span {
        start: CLOCK_HEADER_START.into(),
        end: CLOCK_HEADER_END.into(),
    });
    pieces.push(Protected::Span {
        start: RUNTIME_LINE_START.into(),
        end: RUNTIME_LINE_END.into(),
    });
    pieces
}

/// Every code-owned text a chat request may carry unmasked, for the
/// scanner's exemptions: the protected pieces plus what the clock header
/// and runtime line render (templates, weekday and month names, `zone`).
pub fn code_owned_texts(zone: &str) -> Vec<String> {
    let mut texts = fixed();
    texts.push(clock_header_template());
    texts.push(runtime_line(""));
    texts.push(focus_line("x"));
    texts.extend(WEEKDAYS.map(str::to_owned));
    texts.extend(MONTHS.map(str::to_owned));
    texts.push(zone.to_owned());
    texts
}

/// The header's fixed words (its dates and zone are added separately).
fn clock_header_template() -> String {
    let now = chrono::DateTime::from_timestamp(0, 0).expect("epoch");
    clock_header(&now, chrono_tz::UTC, &now)
}
