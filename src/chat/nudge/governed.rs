//! The production rewriter: one request through a governed rewrite session
//! (`ModelClient::open_rewrite`: `try_acquire`, non-waiting rate token, no
//! retries), on the route snapshot read for the request. Persona text, seed,
//! names and URLs are sent unchanged.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use super::{
    prompt::RewritePrompt,
    rewrite::{NudgeRewriter, RewriteFailure},
};
use crate::infrastructure::llm::governor::{ModelClient, Role, SessionError, SessionFailure};
use crate::infrastructure::llm::{ChatRequest, ErrorCode, LlmProvider, Message};

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
        let route = self
            .client
            .governor()
            .route(Role::Rewrite)
            .ok_or(RewriteFailure::Misconfigured)?;
        let messages = vec![
            Message::System {
                content: prompt.system().to_owned(),
            },
            Message::User {
                content: prompt.seed().to_owned(),
            },
        ];
        let mut session = self
            .client
            .open_rewrite_on(&route, "nudge", deadline)
            .map_err(|error| classify(&error))?;
        let request = ChatRequest {
            model: route.alias.clone(),
            messages,
            tools: Vec::new(),
            output_schema: None,
            // Read once with the route: a save applies to the next rewrite.
            max_output_tokens: self
                .reserve
                .as_ref()
                .map_or(REWRITE_MAX_OUTPUT_TOKENS, |reserve| reserve(&route.alias)),
            // The rewrite role's live level (read once, with the alias).
            reasoning: route.effort,
            sampling: None,
        };
        let response = session
            .complete(&request)
            .await
            .map_err(|error| classify(&error))?;
        match response.content {
            Some(text) if !text.trim().is_empty() => Ok(text),
            _ => Err(RewriteFailure::Refused),
        }
    }
}

/// Object-safe form of [`NudgeRewriter`], so a pipeline can hold any rewriter
/// without another generic parameter.
pub trait DynRewrite: Send + Sync {
    fn rewrite_boxed<'a>(
        &'a self,
        prompt: &'a RewritePrompt,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<String, RewriteFailure>> + Send + 'a>>;
}

impl<T: NudgeRewriter> DynRewrite for T {
    fn rewrite_boxed<'a>(
        &'a self,
        prompt: &'a RewritePrompt,
        deadline: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<String, RewriteFailure>> + Send + 'a>> {
        Box::pin(self.rewrite(prompt, deadline))
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
        self.0.rewrite_boxed(prompt, deadline).await
    }
}
