//! `extractions.json`: the Extractions log list, its per-model summary and
//! the call.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;

use super::{LogFacets, Names, UsageSummary, UsageTally};
use crate::{
    api::dto::iso_instant,
    domain::{
        drafts::{DraftStatus, LoadedDraft},
        ids::short_id,
        members::member_name,
        model_log::{ExtractionLog, WatchedMessage},
        proposals::StoredCard,
    },
    extract::pipeline::{HISTORY_UNREADABLE, SCHEDULE_UNREADABLE},
};

/// Query params of `GET /api/admin/extractions`, all optional and
/// combinable: `model`, `from`/`to` (guild-local YYYY-MM-DD), `outcome`
/// (comma-separated, any of), `channel`, `member`, `q`. Unknown outcomes or
/// malformed dates: 422 invalid_filter.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Extractions {
    pub model: String,
    /// Per model over the filtered calls.
    pub summary: Vec<ExtractionSummary>,
    pub rows: Vec<ExtractionRow>,
    /// Rows before filtering.
    pub total: u64,
    pub facets: LogFacets,
}

/// Per model over the listed (filtered) extraction calls.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ExtractionSummary {
    pub model: String,
    pub count: usize,
    #[serde(flatten)]
    pub usage: UsageSummary,
}

/// The fields a row and the call share.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ExtractionBase {
    pub id: String,
    pub short_id: String,
    pub at: String,
    pub model: String,
    pub latency_ms: Option<u64>,
    pub channel: Option<String>,
    pub channel_id: String,
    pub error: Option<String>,
    #[cfg_attr(test, ts(type = "ExtractionOutcome"))]
    pub outcome: &'static str,
    /// Provider-reported tokens summed over the call's reporting attempts; null = not reported (never 0).
    #[cfg_attr(test, ts(optional = nullable))]
    pub prompt_tokens: Option<u64>,
    #[cfg_attr(test, ts(optional = nullable))]
    pub completion_tokens: Option<u64>,
    /// Provider-reported reasoning tokens over reporting attempts; null = unknown.
    #[cfg_attr(test, ts(optional = nullable))]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ExtractionRow {
    #[serde(flatten)]
    pub base: ExtractionBase,
    pub messages: usize,
    pub changes: usize,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Amendment {
    pub kind: String,
    pub bosses: String,
    pub when: String,
    pub confidence: f64,
    pub status: &'static str,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReadMessage {
    pub id: String,
    pub author: String,
    #[cfg_attr(test, ts(as = "Option<_>", optional))]
    pub author_id: String,
    pub at: String,
    pub content: String,
}

/// The context the call was budgeted for.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct CallContext {
    pub window: u64,
    pub reserve: u64,
    pub source: String,
}

/// A change the scheduler refused to stage for the call.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ExtractionRefusal {
    pub change: String,
    pub code: String,
    pub message: String,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Extraction {
    #[serde(flatten)]
    pub base: ExtractionBase,
    pub prompt: String,
    pub raw_response: String,
    /// Response-only text, capped at 64 KiB including a visible marker.
    #[cfg_attr(test, ts(optional = nullable))]
    pub reasoning_content: Option<String>,
    /// Local prompt estimate over the attempts that reported usage, else every sent attempt.
    #[cfg_attr(test, ts(optional = nullable))]
    pub prompt_estimate: Option<u64>,
    /// Null when not logged whole.
    #[cfg_attr(test, ts(optional = nullable))]
    pub context: Option<CallContext>,
    pub amendments: Vec<Amendment>,
    pub messages: Vec<ReadMessage>,
    #[cfg_attr(test, ts(as = "Option<_>", optional))]
    pub refusals: Vec<ExtractionRefusal>,
    /// The call session's gateway correlation stem; null when not recorded.
    #[cfg_attr(test, ts(optional = nullable))]
    pub session_id: Option<String>,
    /// Every `x-request-id` the call sent, in order (retries included);
    /// empty when none was recorded.
    #[cfg_attr(test, ts(as = "Option<_>", optional))]
    pub request_ids: Vec<String>,
}

/// Per model over the listed calls: how many, and their reported usage
/// (each call's pair is summed over its reporting attempts).
pub fn extraction_summary(rows: &[ExtractionLog]) -> Vec<ExtractionSummary> {
    let listed: BTreeSet<&str> = rows.iter().map(|log| log.model.as_str()).collect();
    listed
        .into_iter()
        .map(|model| {
            let mine: Vec<&ExtractionLog> = rows.iter().filter(|log| log.model == model).collect();
            let mut usage = UsageTally::default();
            for log in &mine {
                usage.add(
                    log.prompt_tokens,
                    log.completion_tokens,
                    log.prompt_estimate,
                );
            }
            ExtractionSummary {
                model: model.to_owned(),
                count: mine.len(),
                usage: usage.summary(),
            }
        })
        .collect()
}

/// Shown instead of a logged failure that is not a known typed text.
pub const CALL_FAILED: &str = "The call failed; the server log has the detail.";

/// Fixed texts the extractor logs (its own sentences, governor refusals,
/// session failures, schema and date errors).
fn fixed_failures() -> Vec<String> {
    use crate::infrastructure::llm::governor::Refused;
    let refused = [
        Refused::UnknownRole,
        Refused::Ungrouped,
        Refused::ExternalForbidden,
        Refused::MustNotWait,
        Refused::Busy,
        Refused::Timeout,
        Refused::Unavailable { retry_at: None },
        Refused::RateLimited {
            wait: std::time::Duration::ZERO,
        },
        Refused::RetryBudgetExhausted,
    ];
    [
        SCHEDULE_UNREADABLE,
        HISTORY_UNREADABLE,
        "no answer",
        "the extraction model is not configured",
        "the model returned an empty response",
        "model request cap reached",
        "model session has ended",
        "clean retry unavailable",
        "answer retry unavailable",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(refused.iter().map(ToString::to_string))
    .chain([crate::domain::time::DateOutOfRange.to_string()])
    .collect()
}

/// Typed texts with a variable part that is never store or backend text:
/// redacted provider errors, cut-off replies (counts only), timeouts, schema
/// validation and identity decoding (model output), and the external-route
/// refusal (config).
fn typed_failure(text: &str) -> bool {
    const PREFIXES: [&str; 6] = [
        "LLM completion failed (",
        "Incomplete: reply ",
        "the model did not answer within ",
        "not JSON: ",
        "expected a JSON object, got ",
        "unknown identity token at byte ",
    ];
    let first = text.lines().next().unwrap_or_default();
    let validation = first.split_once(' ').is_some_and(|(count, rest)| {
        !count.is_empty()
            && count.bytes().all(|byte| byte.is_ascii_digit())
            && matches!(
                rest,
                "validation error for Extraction" | "validation errors for Extraction"
            )
    });
    PREFIXES.iter().any(|prefix| text.starts_with(prefix))
        || validation
        || (text.starts_with("role ") && text.ends_with(" but pseudonymization is off"))
}

/// The logged failure when it is a known typed text, else [`CALL_FAILED`]:
/// store and backend text (paths, SQLite messages), including rows written
/// before the extractor stopped logging it, never reaches the portal.
pub fn call_error(error: Option<&str>) -> Option<String> {
    let error = error?;
    Some(
        if typed_failure(error) || fixed_failures().iter().any(|known| known == error) {
            error.to_owned()
        } else {
            CALL_FAILED.to_owned()
        },
    )
}

fn base(names: &Names<'_>, log: &ExtractionLog) -> ExtractionBase {
    ExtractionBase {
        id: log.id.clone(),
        short_id: short_id(&log.id),
        at: iso_instant(log.at),
        model: log.model.clone(),
        latency_ms: log.latency_ms,
        channel: names.channel(log.channel_id.as_deref()),
        channel_id: log.channel_id.clone().unwrap_or_default(),
        error: call_error(log.error.as_deref()),
        outcome: log.outcome.as_str(),
        prompt_tokens: log.prompt_tokens,
        completion_tokens: log.completion_tokens,
        reasoning_tokens: log.reasoning_tokens,
    }
}

/// The call's resolved context (`guardrail.context`) when it was logged
/// whole, else `None`.
fn call_context(log: &ExtractionLog) -> Option<CallContext> {
    let context = log.guardrail.get("context");
    let field = |name: &str| context.and_then(|context| context.get(name));
    Some(CallContext {
        window: field("window").and_then(Value::as_u64)?,
        reserve: field("reserve").and_then(Value::as_u64)?,
        source: field("source").and_then(Value::as_str)?.to_owned(),
    })
}

pub fn extraction_row(names: &Names<'_>, log: &ExtractionLog) -> ExtractionRow {
    ExtractionRow {
        base: base(names, log),
        messages: log.message_ids.len(),
        changes: log.proposal_ids.len(),
    }
}

/// A proposal's state in v4's words where it had one.
fn proposal_status(loaded: Option<&LoadedDraft>) -> &'static str {
    let Some(loaded) = loaded else {
        return "missing";
    };
    match loaded.draft.status {
        DraftStatus::Submitted | DraftStatus::Open => "proposed",
        DraftStatus::Merged => "confirmed",
        DraftStatus::Rejected => "rejected",
        DraftStatus::Expired => "expired",
        DraftStatus::Withdrawn => "withdrawn",
        DraftStatus::Discarded
            if loaded.draft.close_reason.as_deref() == Some(crate::domain::drafts::SUPERSEDED) =>
        {
            "superseded"
        }
        DraftStatus::Discarded => "discarded",
    }
}

pub struct Proposed<'a> {
    pub card: Option<&'a StoredCard>,
    pub draft: Option<&'a LoadedDraft>,
}

fn amendment(zone: chrono_tz::Tz, proposed: &Proposed<'_>) -> Amendment {
    let status = proposal_status(proposed.draft);
    let Some(card) = proposed.card else {
        let kind = proposed
            .draft
            .and_then(|loaded| loaded.draft.subject.as_deref())
            .and_then(crate::domain::proposals::ProposalSubject::parse)
            .map_or_else(
                || "change".to_owned(),
                |subject| subject.kind.as_str().to_owned(),
            );
        return Amendment {
            kind,
            bosses: String::new(),
            when: String::new(),
            confidence: 0.0,
            status,
        };
    };
    let details = &card.details;
    let when = match details.new_datetime {
        Some(at) => crate::api::dto::when(at, zone),
        None => [details.day_ref.as_deref(), details.time_ref.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" "),
    };
    Amendment {
        kind: details.kind.as_str().to_owned(),
        bosses: details.bosses.join(", "),
        when,
        confidence: details.confidence,
        status,
    }
}

/// The call: prompt, raw response, what it proposed, the messages it read
/// (in the log's order; pruned ones are left out) and the changes refused
/// up front.
pub fn extraction(
    names: &Names<'_>,
    zone: chrono_tz::Tz,
    log: &ExtractionLog,
    proposed: &[Proposed<'_>],
    messages: &[WatchedMessage],
) -> Extraction {
    let read = log
        .message_ids
        .iter()
        .filter_map(|id| messages.iter().find(|message| &message.id == id))
        .map(|message| ReadMessage {
            id: message.id.clone(),
            author: member_name(names.roster, &message.author_id),
            author_id: message.author_id.clone(),
            at: iso_instant(message.created_at),
            content: message.content.clone(),
        })
        .collect();
    Extraction {
        base: base(names, log),
        prompt: log.prompt.clone(),
        raw_response: log.raw_response.clone(),
        reasoning_content: log.reasoning_content.clone(),
        prompt_estimate: log.prompt_estimate,
        context: call_context(log),
        amendments: proposed
            .iter()
            .map(|proposed| amendment(zone, proposed))
            .collect(),
        messages: read,
        refusals: log
            .refusals
            .iter()
            .map(|refusal| ExtractionRefusal {
                change: refusal.change.clone(),
                code: refusal.code.clone(),
                message: refusal.message.clone(),
            })
            .collect(),
        session_id: log.session_id.clone(),
        request_ids: log.request_ids.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::scheduler::StoreError;

    #[test]
    fn only_known_typed_failures_are_shown() {
        for shown in [
            "no answer",
            "model unavailable",
            SCHEDULE_UNREADABLE,
            "date value out of range",
            "LLM completion failed (InvalidOutput, digest=00000000000000ff)",
            "Incomplete: reply cut off at the token limit (finish=length, 6000 of 6000 tokens, 5800 reasoning)",
            "the model did not answer within 60s",
            "2 validation errors for Extraction\namendments.0.kind\n  Input should be ...",
            "role extraction routes to external model \"x\" but pseudonymization is off",
        ] {
            assert_eq!(call_error(Some(shown)).as_deref(), Some(shown), "{shown}");
        }
        for hidden in [
            StoreError::Backend("/private/var/db/kanade.sqlite3: disk I/O error".into())
                .to_string(),
            "schedule constraint violated: runs.id".into(),
            "v4: Traceback (most recent call last)".into(),
            "1 validation error for Extraction/private".into(),
        ] {
            assert_eq!(
                call_error(Some(&hidden)).as_deref(),
                Some(CALL_FAILED),
                "{hidden}"
            );
        }
        assert_eq!(call_error(None), None);
    }
}
