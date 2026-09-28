//! The optional live rewrite of a seed line by the small `rewrite` model, and
//! the checks its output must pass before it replaces the seed.

use std::{future::Future, time::Duration};

use super::{prompt::RewritePrompt, safety};
use crate::chat::persona::{NUDGE_FIELDS, check_nudge_line};

/// User decision: the rewrite gets ~2 s, then the seed line is used.
pub const REWRITE_DEADLINE: Duration = Duration::from_secs(2);

/// Why a rewriter produced no line. Every variant falls back to the seed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RewriteFailure {
    /// Not attempted or failed: no permit, breaker open, role unset, transport
    /// or model error.
    Unavailable,
    /// The provider declined: a content-filter finish, a refusal, or an
    /// empty/incomplete reply. Adapters must map these here, never to text.
    Refused,
    /// A deployment or request prevents it (unknown or ungrouped role, a
    /// rejected key or request);
    /// kept apart so logs can flag it.
    Misconfigured,
}

/// A governed one-line rewrite. Implementations take the `rewrite` role's
/// permit with `try_acquire` only (never queue), send at most one request with
/// no retries, and give up by `deadline`; the caller also enforces it.
pub trait NudgeRewriter: Send + Sync {
    fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = Result<String, RewriteFailure>> + Send;
}

impl<T: NudgeRewriter + ?Sized> NudgeRewriter for &T {
    fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = Result<String, RewriteFailure>> + Send {
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
    ) -> Result<String, RewriteFailure> {
        Err(RewriteFailure::Unavailable)
    }
}

/// Why a rewrite was not used; loggable, never carries the rewritten text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// Fails the seed-line rules: one line, ≤140 chars, no mentions, links,
    /// URLs or unknown placeholders.
    LineRules,
    /// Not the seed's placeholder multiset.
    Placeholders,
    /// Markdown, a Unicode format character or an invite link.
    Markup,
    /// A deny-listed word (the matched entry, not the line).
    Denied(&'static str),
}

/// The model's line if it passes every check. Surrounding whitespace is trimmed first.
pub fn accept_rewrite(output: &str, seed: &str) -> Result<String, Rejection> {
    let line = output.trim();
    check_nudge_line(line).map_err(|_| Rejection::LineRules)?;
    if safety::has_markup(line) || safety::has_format_char(line) || safety::has_invite(line) {
        return Err(Rejection::Markup);
    }
    let same_fields = NUDGE_FIELDS
        .iter()
        .all(|field| line.matches(field).count() == seed.matches(field).count());
    if !same_fields {
        return Err(Rejection::Placeholders);
    }
    if let Some(word) = safety::denied_word(line) {
        return Err(Rejection::Denied(word));
    }
    Ok(line.to_owned())
}
