//! The production rewriter: one request through a governed rewrite session
//! (`ModelClient::open_rewrite`: `try_acquire`, non-waiting rate token, no
//! retries), on the route snapshot read for the request. Persona text, seed,
//! names and URLs are sent unchanged.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use super::{
    prompt::RewritePrompt,
    rewrite::{NudgeRewriter, RewriteDetail, RewriteFailure, RewriteOutcome},
};
use crate::infrastructure::llm::governor::{ModelClient, Role, SessionError, SessionFailure};
use crate::infrastructure::llm::{ChatRequest, ErrorCode, LlmProvider};

/// One short line; a few tokens of slack for the model's wording. The
/// default rewrite reserve when no live context resolver is attached.
pub const REWRITE_MAX_OUTPUT_TOKENS: u32 = 96;

/// The rewrite reserve (`max_tokens`) for an alias, read per request.
pub type RewriteReserve = Arc<dyn Fn(&str) -> u32 + Send + Sync>;

pub struct GovernedRewriter<P> {
    client: Arc<ModelClient<P>>,
    reserve: Option<RewriteReserve>,
}

impl<P> std::fmt::Debug for GovernedRewriter<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GovernedRewriter").finish_non_exhaustive()
    }
}

impl<P> GovernedRewriter<P> {
    pub fn new(client: Arc<ModelClient<P>>) -> Self {
        Self {
            client,
            reserve: None,
        }
    }

    /// Size each request from live settings (the resolved rewrite reserve,
    /// already clamped to the route's published output maximum).
    #[must_use]
    pub fn with_reserve(mut self, reserve: RewriteReserve) -> Self {
        self.reserve = Some(reserve);
        self
    }
}

/// Content filter, cut-off and empty replies are the provider declining.
fn classify(error: &SessionError) -> RewriteFailure {
    if error.is_misconfiguration() {
        return RewriteFailure::Misconfigured;
    }
    match &error.failure {
        SessionFailure::Model(error)
            if matches!(
                error.code,
                ErrorCode::ContentFiltered | ErrorCode::Incomplete
            ) =>
        {
            RewriteFailure::Refused
        }
        _ => RewriteFailure::Unavailable,
    }
}

impl<P: LlmProvider> NudgeRewriter for GovernedRewriter<P> {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        self.rewrite_detailed(prompt, deadline).await.result
    }

    async fn rewrite_detailed(&self, prompt: &RewritePrompt, deadline: Duration) -> RewriteOutcome {
        let Some(route) = self.client.governor().route(Role::Rewrite) else {
            return RewriteOutcome {
                result: Err(RewriteFailure::Misconfigured),
                detail: RewriteDetail {
                    code: Some("no_route"),
                    ..RewriteDetail::default()
                },
            };
        };
        // Read once with the route: a save applies to the next rewrite.
        let max_output_tokens = self
            .reserve
            .as_ref()
            .map_or(REWRITE_MAX_OUTPUT_TOKENS, |reserve| reserve(&route.alias));
        let mut detail = RewriteDetail {
            alias: Some(route.alias.clone()),
            // The rewrite role's live level (read once, with the alias).
            effort: route.effort,
            max_output_tokens: Some(max_output_tokens),
            ..RewriteDetail::default()
        };
        let mut session = match self.client.open_rewrite_on(&route, "nudge", deadline) {
            Ok(session) => session,
            Err(error) => {
                detail.code = Some(error.code());
                return RewriteOutcome {
                    result: Err(classify(&error)),
                    detail,
                };
            }
        };
        let request = ChatRequest {
            model: route.alias.clone(),
            messages: prompt.messages(),
            tools: Vec::new(),
            output_schema: None,
            max_output_tokens,
            reasoning: route.effort,
            sampling: None,
        };
        let response = session.complete(&request).await;
        if let Some(sent) = session.last_sent() {
            detail.alias = Some(sent.alias.clone());
            detail.effort = sent.effort;
        }
        detail.reservation = session.last_reservation();
        detail.budget = session.over_budget();
        detail.request_id = session.request_ids().last().cloned();
        let result = match response {
            Ok(response) => {
                detail.usage = response.usage;
                detail.reasoning_tokens = response.reasoning_tokens;
                detail.reasoning = response.reasoning_content;
                detail.reply = response.content.clone();
                match response.content {
                    Some(text) if !text.trim().is_empty() => Ok(text),
                    _ => {
                        detail.code = Some("empty_reply");
                        Err(RewriteFailure::Refused)
                    }
                }
            }
            Err(error) => {
                detail.code = Some(error.code());
                // A reply refused for exceeding its reservation still shows
                // what the model reported.
                if let Some(overrun) = session.overrun() {
                    detail.usage = overrun.usage.clone();
                    detail.reasoning_tokens = overrun.reasoning_tokens;
                    detail.reasoning = overrun.reasoning_content.clone();
                    detail.reply = overrun.content.clone();
                }
                Err(classify(&error))
            }
        };
        RewriteOutcome { result, detail }
    }
}

/// Object-safe form of [`NudgeRewriter`], so a pipeline can hold any rewriter
/// without another generic parameter.
pub trait DynRewrite: Send + Sync {
    fn rewrite_boxed<'a>(
        &'a self,
        prompt: &'a RewritePrompt,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = RewriteOutcome> + Send + 'a>>;
}

impl<T: NudgeRewriter> DynRewrite for T {
    fn rewrite_boxed<'a>(
        &'a self,
        prompt: &'a RewritePrompt,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = RewriteOutcome> + Send + 'a>> {
        Box::pin(self.rewrite_detailed(prompt, deadline))
    }
}

/// A shared, type-erased rewriter.
#[derive(Clone)]
pub struct SharedRewriter(pub Arc<dyn DynRewrite>);

impl std::fmt::Debug for SharedRewriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedRewriter").finish_non_exhaustive()
    }
}

impl NudgeRewriter for SharedRewriter {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        self.0.rewrite_boxed(prompt, deadline).await.result
    }

    async fn rewrite_detailed(&self, prompt: &RewritePrompt, deadline: Duration) -> RewriteOutcome {
        self.0.rewrite_boxed(prompt, deadline).await
    }
}
