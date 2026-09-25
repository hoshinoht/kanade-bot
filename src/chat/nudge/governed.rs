//! The production rewriter: one request through a governed rewrite session
//! (`ModelClient::open_rewrite`: `try_acquire`, non-waiting rate token, no
//! retries), behind the identity route guard.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use super::{
    prompt::RewritePrompt,
    rewrite::{NudgeRewriter, RewriteFailure},
};
use crate::infrastructure::llm::governor::{ModelClient, Role, SessionError, SessionFailure};
use crate::infrastructure::llm::identity::{IdentityCodec, open_session};
use crate::infrastructure::llm::{ChatRequest, ErrorCode, LlmProvider};

/// One short line; a few tokens of slack for the model's wording.
pub const REWRITE_MAX_OUTPUT_TOKENS: u32 = 96;

pub struct GovernedRewriter<P> {
    client: Arc<ModelClient<P>>,
    codec: Arc<dyn IdentityCodec>,
}

impl<P> std::fmt::Debug for GovernedRewriter<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GovernedRewriter").finish_non_exhaustive()
    }
}

impl<P> GovernedRewriter<P> {
    pub fn new(client: Arc<ModelClient<P>>, codec: Arc<dyn IdentityCodec>) -> Self {
        Self { client, codec }
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
        // The prompt carries no member data, so the roster is empty; the guard
        // still refuses an external route while pseudonymisation is off, which
        // is an operator setting like the governor's own `ExternalForbidden`.
        let _identity = open_session(self.codec.as_ref(), route, &[])
            .map_err(|_| RewriteFailure::Misconfigured)?;
        let mut session = self
            .client
            .open_rewrite("nudge", deadline)
            .map_err(|error| classify(&error))?;
        let request = ChatRequest {
            model: route.alias.clone(),
            messages: prompt.messages(),
            tools: Vec::new(),
            output_schema: None,
            max_output_tokens: REWRITE_MAX_OUTPUT_TOKENS,
            reasoning: None,
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
