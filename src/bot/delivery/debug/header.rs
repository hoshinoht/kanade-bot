//! `/debug header`'s public report: each rewrite try's line (in a code span,
//! so it can neither ping nor format), its verdict and latency.

use std::time::Duration;

use super::TEST_PREFIX;
use crate::bot::commands::{CONTENT_LIMIT, HeaderTrialKind};
use crate::bot::delivery::cards::{Trial, Verdict};

/// Shown characters of one try's line.
const LINE_CHARS: usize = 200;

/// A verdict in words: `accepted`, `rejected (markup)`, `timeout`, …
pub fn verdict_text(verdict: Verdict) -> String {
    match verdict {
        Verdict::Accepted => "accepted".to_owned(),
        Verdict::Rejected(_) | Verdict::PhraseRejected(_) => {
            format!("rejected ({})", verdict.rule().unwrap_or("gate"))
        }
        Verdict::NoRewriter => "no rewriter".to_owned(),
        Verdict::NoPersona => "no persona".to_owned(),
        Verdict::Timeout | Verdict::Failed(_) => verdict.reason().to_owned(),
    }
}

fn mark(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Accepted => "✅",
        Verdict::Rejected(_) | Verdict::PhraseRejected(_) => "❌",
        Verdict::Timeout => "⏱️",
        _ => "⚠️",
    }
}

/// `text` as one inline code span: backticks and control characters
/// replaced, clipped to [`LINE_CHARS`].
pub fn code_span(text: &str) -> String {
    let clean: String = text
        .trim()
        .chars()
        .map(|character| match character {
            '`' => 'ˋ',
            character if character.is_control() => ' ',
            character => character,
        })
        .collect();
    if clean.trim().is_empty() {
        return "(empty)".to_owned();
    }
    let mut shown: String = clean.chars().take(LINE_CHARS).collect();
    if clean.chars().count() > LINE_CHARS {
        shown.push('…');
    }
    format!("`{shown}`")
}

/// The whole report, within Discord's content limit.
pub fn report(kind: HeaderTrialKind, tries: &[(Trial, Duration)]) -> String {
    let accepted = tries
        .iter()
        .filter(|(trial, _)| trial.verdict == Verdict::Accepted)
        .count();
    let mut lines = vec![format!(
        "{TEST_PREFIX}`{}` header rewrite · {} tr{} · {accepted} accepted",
        kind.as_str(),
        tries.len(),
        if tries.len() == 1 { "y" } else { "ies" }
    )];
    for (index, (trial, latency)) in tries.iter().enumerate() {
        let line = trial
            .output
            .as_deref()
            .map(|output| format!(" · {}", code_span(output)))
            .unwrap_or_default();
        lines.push(format!(
            "{}. {} {} · {} ms{line}",
            index + 1,
            mark(trial.verdict),
            verdict_text(trial.verdict),
            latency.as_millis()
        ));
    }
    lines.join("\n").chars().take(CONTENT_LIMIT).collect()
}
