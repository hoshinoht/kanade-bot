//! The optional live rewrite of a seed line by the small `rewrite` model, and
//! the checks its output must pass before it replaces the seed.

use std::{future::Future, time::Duration};

use super::prompt::RewritePrompt;
use crate::chat::persona::{NUDGE_FIELDS, check_nudge_line};

/// User decision: the rewrite gets ~2 s, then the seed line is used.
pub const REWRITE_DEADLINE: Duration = Duration::from_secs(2);

/// The rewrite was not attempted or produced nothing (no permit, breaker
/// open, role unset, external route without pseudonymisation, model error).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RewriteUnavailable;

/// A governed one-line rewrite. Implementations take the `rewrite` role's
/// permit with `try_acquire` only (never queue), send at most one request with
/// no retries, and give up by `deadline`; the caller also enforces it.
pub trait NudgeRewriter: Send + Sync {
    fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = Result<String, RewriteUnavailable>> + Send;
}

impl<T: NudgeRewriter + ?Sized> NudgeRewriter for &T {
    fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = Result<String, RewriteUnavailable>> + Send {
        (**self).rewrite(prompt, deadline)
    }
}

/// No rewrite role configured: seeds are used as-is.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoRewrite;

impl NudgeRewriter for NoRewrite {
    async fn rewrite(
        &self,
        _prompt: &RewritePrompt,
        _deadline: Duration,
    ) -> Result<String, RewriteUnavailable> {
        Err(RewriteUnavailable)
    }
}

/// The model's line if it passes the seed-line rules (one line, ≤140 chars,
/// no mentions, links or URLs, only `{boss}`/`{day}`/`{time}`) and keeps
/// exactly the seed's placeholders. Surrounding whitespace is trimmed first.
pub fn accept_rewrite(output: &str, seed: &str) -> Option<String> {
    let line = output.trim();
    check_nudge_line(line).ok()?;
    let same_fields = NUDGE_FIELDS
        .iter()
        .all(|field| line.contains(field) == seed.contains(field));
    same_fields.then(|| line.to_owned())
}
