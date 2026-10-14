//! The optional live rewrite of a seed line by the small `rewrite` model, and
//! the checks its output must pass before it replaces the seed.

use std::{future::Future, time::Duration};

use super::{
    prompt::RewritePrompt,
    safety::{self, Hit, WordFilter},
};
use crate::chat::persona::{NUDGE_FIELDS, check_nudge_line};
use crate::infrastructure::llm::{Effort, Usage};

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

/// What the governed layer saw of one rewrite call, for the Rewrites log.
/// Every field is optional: a call refused before sending has only `code`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RewriteDetail {
    /// The specific failure (`SessionError::code`, `empty_reply`,
    /// `no_route`, `shutdown`); `None` on success.
    pub code: Option<&'static str>,
    /// The alias and reasoning effort as sent (else as requested).
    pub alias: Option<String>,
    pub effort: Option<Effort>,
    /// Provider-reported usage, also for a reply refused for exceeding
    /// `reservation`.
    pub usage: Option<Usage>,
    pub reasoning_tokens: Option<u64>,
    /// The model's raw reply text and its reasoning text.
    pub reply: Option<String>,
    pub reasoning: Option<String>,
    /// The runner's token reservation (prompt estimate + the requested
    /// reserve, whether or not `max_tokens` went out).
    pub reservation: Option<u32>,
    /// The call token budget the reservation exceeded, when the runner
    /// refused it before sending.
    pub budget: Option<u32>,
    /// The `max_tokens` the request carried; `None` when nothing was sent
    /// or the route has no sampling controls (the body omits it).
    pub max_output_tokens: Option<u32>,
    /// The `x-request-id` the request carried.
    pub request_id: Option<String>,
}

/// A rewrite's result with its [`RewriteDetail`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RewriteOutcome {
    pub result: Result<String, RewriteFailure>,
    pub detail: RewriteDetail,
}

impl RewriteOutcome {
    /// A result with nothing known about the call.
    pub fn plain(result: Result<String, RewriteFailure>) -> Self {
        Self {
            result,
            detail: RewriteDetail::default(),
        }
    }
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

    /// As [`Self::rewrite`], with what the call reported. Wrappers must
    /// forward it so no layer drops the detail.
    fn rewrite_detailed(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = RewriteOutcome> + Send {
        let call = self.rewrite(prompt, deadline);
        async move { RewriteOutcome::plain(call.await) }
    }
}

impl<T: NudgeRewriter + ?Sized> NudgeRewriter for &T {
    fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = Result<String, RewriteFailure>> + Send {
        (**self).rewrite(prompt, deadline)
    }

    fn rewrite_detailed(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> impl Future<Output = RewriteOutcome> + Send {
        (**self).rewrite_detailed(prompt, deadline)
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
    /// A deny-listed word (the matched built-in entry, not the line; an
    /// admin-added word reports [`CUSTOM_WORD`]).
    Denied(&'static str),
}

impl Rejection {
    /// The gate rule in words, for logs and `/debug` (never the line).
    pub fn rule(self) -> &'static str {
        match self {
            Self::LineRules => "line rules",
            Self::Placeholders => "placeholders",
            Self::Markup => "markup",
            Self::Denied(_) => "deny-listed word",
        }
    }
}

/// What [`Rejection::Denied`] names for an admin-added word.
pub const CUSTOM_WORD: &str = "custom";

/// The model's line if it passes every check against the built-in deny-list.
pub fn accept_rewrite(output: &str, seed: &str) -> Result<String, Rejection> {
    accept_rewrite_with(output, seed, WordFilter::builtin())
}

/// As [`accept_rewrite`], against the live effective deny-list. Surrounding
/// whitespace is trimmed first.
pub fn accept_rewrite_with(
    output: &str,
    seed: &str,
    words: &WordFilter,
) -> Result<String, Rejection> {
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
    match words.denied(line) {
        Some(Hit::Builtin(word)) => return Err(Rejection::Denied(word)),
        Some(Hit::Extra(_)) => return Err(Rejection::Denied(CUSTOM_WORD)),
        None => {}
    }
    Ok(line.to_owned())
}
