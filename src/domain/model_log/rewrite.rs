//! The Rewrites log: one row per persona rewrite attempt (reminder headers
//! ahead of their send, `/debug` trials, self-service nudges) with the
//! verdict, the gate rule or error code behind it, the route and the reply.
//! Rows hold the persona prompt's seed and the model's own text only, never
//! member names or ids.

use std::future::Future;

use chrono::{DateTime, Utc};

use super::filter::{LogCursor, LogPage};
use super::reasoning::REASONING_CAP;
use super::records::is_correlation_id;
use crate::domain::scheduler::StoreError;

/// Which line a rewrite was for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RewriteKind {
    DayOf,
    Countdown,
    Digest,
    Nudge,
}

impl RewriteKind {
    pub const ALL: [Self; 4] = [Self::DayOf, Self::Countdown, Self::Digest, Self::Nudge];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DayOf => "day_of",
            Self::Countdown => "countdown",
            Self::Digest => "digest",
            Self::Nudge => "nudge",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// Where the rewrite ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RewriteStage {
    /// The daily reminder-header batch (and its retries).
    Batch,
    /// A reminder header the last batch had not seen (a run added or moved since).
    Catchup,
    /// `/debug header` and `/debug ping header:rewrite`.
    Debug,
    /// A chat self-service nudge.
    Nudge,
}

impl RewriteStage {
    pub const ALL: [Self; 4] = [Self::Batch, Self::Catchup, Self::Debug, Self::Nudge];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Batch => "batch",
            Self::Catchup => "catchup",
            Self::Debug => "debug",
            Self::Nudge => "nudge",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.as_str() == text)
    }
}

/// Every verdict a row may carry (`Verdict::reason` and the nudge's mapping).
pub const REWRITE_VERDICTS: [&str; 8] = [
    "accepted",
    "rejected",
    "timeout",
    "unavailable",
    "refused",
    "misconfigured",
    "no_rewriter",
    "no_persona",
];

/// Stored bytes of the model's reply, including [`REPLY_TRUNCATED`].
pub const REPLY_CAP: usize = 8 * 1024;
pub const REPLY_TRUNCATED: &str = "\n… [reply truncated]";
/// Longest context reference, seed and final line.
pub const CONTEXT_CAP: usize = 200;
pub const LINE_CAP: usize = 1024;
/// Longest rule or error code.
pub const CODE_CAP: usize = 64;

/// `text` within [`REPLY_CAP`] bytes with a visible marker; never splits UTF-8.
pub fn capped_reply(text: &str) -> String {
    if text.len() <= REPLY_CAP {
        return text.to_owned();
    }
    let mut end = REPLY_CAP - REPLY_TRUNCATED.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{REPLY_TRUNCATED}", &text[..end])
}

/// One rewrite attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteLog {
    pub id: String,
    pub at: DateTime<Utc>,
    pub kind: RewriteKind,
    pub stage: RewriteStage,
    /// What it was for: a card key, a digest week, a `/debug` command or a
    /// nudge's purpose and mood. Never a member name or id.
    pub context: Option<String>,
    /// One of [`REWRITE_VERDICTS`].
    pub verdict: String,
    /// The gate rule that refused the line (`rejected` only).
    pub rule: Option<String>,
    /// The specific failure (`SessionError::code`, `shutdown`, …).
    pub code: Option<String>,
    pub latency_ms: Option<u64>,
    /// The alias the request named; `None` when nothing was sent.
    pub model: Option<String>,
    /// Reasoning effort as sent.
    pub reasoning: Option<String>,
    /// Provider-reported usage; both or neither.
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// The runner's token reservation (prompt estimate + `max_tokens`) the
    /// reported usage was checked against.
    pub reservation: Option<u64>,
    /// The call token budget a reservation exceeded when the runner refused
    /// it before sending ("reserved N > budget B").
    pub budget: Option<u64>,
    /// `max_tokens` as requested.
    pub max_output_tokens: Option<u64>,
    /// The seed line the model was asked to rewrite.
    pub seed: String,
    /// The model's raw reply, capped at [`REPLY_CAP`].
    pub reply: Option<String>,
    /// Response-only reasoning, capped at [`REASONING_CAP`].
    pub reasoning_content: Option<String>,
    /// The line used: the accepted rewrite or the seed fallback (unfilled).
    pub line: Option<String>,
    /// The `x-request-id` the call sent.
    pub request_id: Option<String>,
}

fn too_long(field: &str, value: Option<&str>, cap: usize) -> Result<(), StoreError> {
    match value {
        Some(text) if text.is_empty() || text.len() > cap => Err(StoreError::Constraint(format!(
            "rewrite {field} must be 1..={cap} bytes"
        ))),
        _ => Ok(()),
    }
}

impl RewriteLog {
    /// The row shape both stores refuse to break.
    pub fn check_shape(&self) -> Result<(), StoreError> {
        if !REWRITE_VERDICTS.contains(&self.verdict.as_str()) {
            return Err(StoreError::Constraint(format!(
                "unknown rewrite verdict {}",
                self.verdict
            )));
        }
        if self.prompt_tokens.is_some() != self.completion_tokens.is_some() {
            return Err(StoreError::Constraint(
                "rewrite token usage is a pair".into(),
            ));
        }
        too_long("context", self.context.as_deref(), CONTEXT_CAP)?;
        too_long("rule", self.rule.as_deref(), CODE_CAP)?;
        too_long("code", self.code.as_deref(), CODE_CAP)?;
        too_long("line", self.line.as_deref(), LINE_CAP)?;
        too_long(
            "reasoning",
            self.reasoning_content.as_deref(),
            REASONING_CAP,
        )?;
        if self.seed.len() > LINE_CAP {
            return Err(StoreError::Constraint("rewrite seed is too long".into()));
        }
        if self
            .reply
            .as_ref()
            .is_some_and(|reply| reply.len() > REPLY_CAP)
        {
            return Err(StoreError::Constraint("rewrite reply is too long".into()));
        }
        if self
            .request_id
            .as_deref()
            .is_some_and(|id| !is_correlation_id(id))
        {
            return Err(StoreError::Constraint(
                "rewrite request id is malformed".into(),
            ));
        }
        Ok(())
    }

    pub fn cursor(&self) -> LogCursor {
        LogCursor {
            at: self.at,
            id: self.id.clone(),
        }
    }
}

/// Rewrites log filters, combined with AND; `verdicts` empty means any. `q`
/// is an ASCII case-insensitive substring of the seed, reply, line,
/// context, rule or code. Pages come back newest first without
/// `reasoning_content` (the detail read has it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteFilter {
    pub model: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub kind: Option<RewriteKind>,
    pub stage: Option<RewriteStage>,
    pub verdicts: Vec<String>,
    pub q: Option<String>,
    pub cursor: Option<LogCursor>,
    pub limit: u32,
}

/// The unfiltered total and the distinct values seen, each sorted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteFacets {
    pub total: u64,
    pub models: Vec<String>,
    pub kinds: Vec<String>,
    pub stages: Vec<String>,
    pub verdicts: Vec<String>,
}

/// Rewrites log persistence: insert-only, pruned with the other model logs
/// by `ModelLogStore::prune_model_logs`.
pub trait RewriteLogStore {
    fn record_rewrite(
        &self,
        log: RewriteLog,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn load_rewrite(
        &self,
        id: &str,
    ) -> impl Future<Output = Result<Option<RewriteLog>, StoreError>> + Send;

    fn list_rewrites(
        &self,
        filter: &RewriteFilter,
    ) -> impl Future<Output = Result<LogPage<RewriteLog>, StoreError>> + Send;

    fn rewrite_facets(&self) -> impl Future<Output = Result<RewriteFacets, StoreError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_cap_includes_marker_and_preserves_utf8() {
        assert_eq!(capped_reply("Waku!"), "Waku!");
        let capped = capped_reply(&"奏".repeat(REPLY_CAP));
        assert!(capped.len() <= REPLY_CAP);
        assert!(capped.ends_with(REPLY_TRUNCATED));
    }

    #[test]
    fn kinds_and_stages_round_trip() {
        for kind in RewriteKind::ALL {
            assert_eq!(RewriteKind::parse(kind.as_str()), Some(kind));
        }
        for stage in RewriteStage::ALL {
            assert_eq!(RewriteStage::parse(stage.as_str()), Some(stage));
        }
        assert_eq!(RewriteKind::parse("send"), None);
    }
}
