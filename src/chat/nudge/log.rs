//! The Rewrites log port: the call site that knows a rewrite's kind, stage
//! and context writes one row per attempt. Logging never fails or changes
//! the caller's outcome: a store error or a slow write is a WARN event and
//! the row is dropped.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use serde_json::json;

use super::rewrite::{RewriteDetail, RewriteFailure};
use crate::domain::ids::{IdGenerator, RandomIds};
use crate::domain::model_log::{
    CONTEXT_CAP, LINE_CAP, RewriteKind, RewriteLog, RewriteLogStore, RewriteStage,
    capped_reasoning, capped_reply, is_correlation_id,
};
use crate::domain::scheduler::StoreError;
use crate::runtime::logging;

/// A row write gives up after this, so a busy store never holds the caller.
pub const LOG_WRITE_DEADLINE: Duration = Duration::from_secs(2);

/// One rewrite attempt as its call site saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteAttempt {
    pub kind: RewriteKind,
    pub stage: RewriteStage,
    /// A card key, digest week, `/debug` command or nudge purpose; never a
    /// member name or id.
    pub context: Option<String>,
    pub seed: String,
    /// One of `REWRITE_VERDICTS`.
    pub verdict: &'static str,
    pub rule: Option<&'static str>,
    /// `None` when no call was attempted.
    pub latency: Option<Duration>,
    /// The line used (accepted rewrite or seed), unfilled.
    pub line: Option<String>,
    pub detail: RewriteDetail,
}

/// The verdict word for a rewriter failure.
pub fn failure_verdict(failure: RewriteFailure) -> &'static str {
    match failure {
        RewriteFailure::Unavailable => "unavailable",
        RewriteFailure::Refused => "refused",
        RewriteFailure::Misconfigured => "misconfigured",
    }
}

/// `text` cut to at most `cap` bytes on a character boundary; empty is `None`.
fn clipped(text: &str, cap: usize) -> Option<String> {
    let mut end = text.len().min(cap);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let text = &text[..end];
    (!text.is_empty()).then(|| text.to_owned())
}

impl RewriteAttempt {
    /// The stored row, every text within its column's bound.
    pub fn into_log(self, id: String, at: DateTime<Utc>) -> RewriteLog {
        let detail = self.detail;
        let (prompt_tokens, completion_tokens) = match &detail.usage {
            Some(usage) => (
                Some(u64::from(usage.prompt_tokens)),
                Some(u64::from(usage.completion_tokens)),
            ),
            None => (None, None),
        };
        RewriteLog {
            id,
            at,
            kind: self.kind,
            stage: self.stage,
            context: self
                .context
                .as_deref()
                .and_then(|context| clipped(context, CONTEXT_CAP)),
            verdict: self.verdict.to_owned(),
            rule: self.rule.map(str::to_owned),
            code: detail.code.map(str::to_owned),
            latency_ms: self
                .latency
                .map(|latency| u64::try_from(latency.as_millis()).unwrap_or(u64::MAX)),
            model: detail.alias,
            reasoning: detail.effort.map(|effort| effort.as_str().to_owned()),
            prompt_tokens,
            completion_tokens,
            reasoning_tokens: detail.reasoning_tokens,
            reservation: detail.reservation.map(u64::from),
            budget: detail.budget.map(u64::from),
            max_output_tokens: detail.max_output_tokens.map(u64::from),
            seed: clipped(&self.seed, LINE_CAP).unwrap_or_default(),
            reply: detail.reply.as_deref().map(capped_reply),
            reasoning_content: detail.reasoning.as_deref().and_then(capped_reasoning),
            line: self
                .line
                .as_deref()
                .and_then(|line| clipped(line, LINE_CAP)),
            request_id: detail.request_id.filter(|id| is_correlation_id(id)),
        }
    }
}

/// Where rewrite attempts are logged.
pub trait RewriteSink: Send + Sync {
    /// Never fails; waits at most [`LOG_WRITE_DEADLINE`].
    fn record(&self, attempt: RewriteAttempt) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

pub type SharedRewriteSink = Arc<dyn RewriteSink>;

/// The Rewrites log over a store, stamping each row with the clock's time
/// and a fresh id.
pub struct StoreRewriteSink<S> {
    store: Arc<S>,
    now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
}

impl<S> StoreRewriteSink<S> {
    pub fn new(store: Arc<S>, now: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>) -> Self {
        Self { store, now }
    }
}

impl<S: RewriteLogStore + Send + Sync> RewriteSink for StoreRewriteSink<S> {
    fn record(&self, attempt: RewriteAttempt) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let log = attempt.into_log(RandomIds.new_id(), (self.now)());
            let write = tokio::time::timeout(LOG_WRITE_DEADLINE, self.store.record_rewrite(log));
            let failure = match write.await {
                Ok(Ok(())) => return,
                Ok(Err(StoreError::Constraint(_))) => "constraint",
                Ok(Err(_)) => "backend",
                Err(_) => "timeout",
            };
            logging::event("WARN", "rewrite_log_failed", json!({"error": failure}));
        })
    }
}
