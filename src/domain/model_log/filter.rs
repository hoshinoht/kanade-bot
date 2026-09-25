//! Log filters and keyset pages, newest first (`at` then `id`, both
//! descending). Every field is optional and they combine with AND.

use chrono::{DateTime, Utc};

use super::outcome::{ChatOutcome, ExtractionOutcome};

/// The largest page a store returns; larger limits are clamped.
pub const MAX_PAGE: u32 = 200;

/// Where the previous page ended: its last item's `at` and `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogCursor {
    pub at: DateTime<Utc>,
    pub id: String,
}

/// Extraction log filters. `from` is inclusive and `to` exclusive (the API
/// maps guild-local dates to instants); `outcomes` empty means any; `q` is
/// an ASCII case-insensitive substring of the prompt or raw response.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractionFilter {
    pub model: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub outcomes: Vec<ExtractionOutcome>,
    pub channel: Option<String>,
    /// Any of the messages' authors.
    pub member: Option<String>,
    pub q: Option<String>,
    pub cursor: Option<LogCursor>,
    /// Page size, clamped to 1..=[`MAX_PAGE`].
    pub limit: u32,
}

/// Chat log filters, as [`ExtractionFilter`] plus `tool` (called in any
/// round) and `min_ms` (total latency at least). `model` matches any
/// round's alias; `q` searches the question and reply. An outcome of
/// `withheld` or `clean_retry` also matches interactions flagged so.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatFilter {
    pub model: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub outcomes: Vec<ChatOutcome>,
    pub channel: Option<String>,
    pub member: Option<String>,
    pub q: Option<String>,
    pub tool: Option<String>,
    pub min_ms: Option<u64>,
    pub cursor: Option<LogCursor>,
    pub limit: u32,
}

/// One page; `next` is present while more items may follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogPage<T> {
    pub items: Vec<T>,
    pub next: Option<LogCursor>,
}

/// The unfiltered total and the distinct values the filter bar offers,
/// each sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LogFacets {
    pub total: u64,
    pub models: Vec<String>,
    /// Chat only (empty for extractions).
    pub tools: Vec<String>,
    pub outcomes: Vec<String>,
    pub channels: Vec<String>,
}

/// The page size a store uses for `limit`.
pub fn page_size(limit: u32) -> u32 {
    limit.clamp(1, MAX_PAGE)
}
