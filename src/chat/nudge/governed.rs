//! The production rewriter: one request through a governed rewrite session
//! (`ModelClient::open_rewrite`: `try_acquire`, non-waiting rate token, no
//! retries), behind the identity route guard. With pseudonymization on, the
//! identity session is opened over the roster ([`GovernedRewriter::with_roster`]),
//! the persona text and seed are encoded (code-owned instruction, moods and
//! `{boss}`/`{day}`/`{time}` stay literal) and the reply is decoded; no
//! roster means no request.

use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use super::{
    prompt::{GENTLE_MOOD, NUDGE_REWRITE_INSTRUCTION, PLAYFUL_MOOD, RewritePrompt, VOICE_LABEL},
    rewrite::{NudgeRewriter, RewriteFailure},
};
use crate::infrastructure::llm::governor::{ModelClient, Role, SessionError, SessionFailure};
use crate::infrastructure::llm::identity::{
    CodecMode, IdentityCodec, Member, Protected, RosterSource, encode_protected, open_session,
};
use crate::infrastructure::llm::{ChatRequest, ErrorCode, LlmProvider, Message};

/// One short line; a few tokens of slack for the model's wording.
pub const REWRITE_MAX_OUTPUT_TOKENS: u32 = 96;

pub struct GovernedRewriter<P> {
    client: Arc<ModelClient<P>>,
    codec: Arc<dyn IdentityCodec>,
    roster: Option<Arc<dyn RosterSource>>,
}

impl<P> std::fmt::Debug for GovernedRewriter<P> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GovernedRewriter").finish_non_exhaustive()
    }
}

impl<P> GovernedRewriter<P> {
    pub fn new(client: Arc<ModelClient<P>>, codec: Arc<dyn IdentityCodec>) -> Self {
        Self {
            client,
            codec,
            roster: None,
        }
    }

    /// The members whose names the persona text or reply may carry; required
    /// while the codec pseudonymizes (without it every rewrite is refused
    /// as misconfigured, and the seed line is used).
    pub fn with_roster(mut self, roster: Arc<dyn RosterSource>) -> Self {
        self.roster = Some(roster);
        self
    }

    /// The roster for this rewrite; `None` (fail closed) when masking needs
    /// one it cannot have.
    fn roster(&self) -> Option<Vec<Member>> {
        let masking = self.codec.mode() == CodecMode::Pseudonymizing;
        let roster = match &self.roster {
            Some(source) => source.roster(),
            None => Some(Vec::new()),
        };
        match roster {
            Some(roster) if masking && roster.is_empty() => None,
            None if masking => None,
            roster => Some(roster.unwrap_or_default()),
        }
    }
}

/// The code-owned pieces of a rewrite prompt, kept literal when encoding.
pub fn protected() -> Vec<Protected> {
    [
        NUDGE_REWRITE_INSTRUCTION,
        PLAYFUL_MOOD,
        GENTLE_MOOD,
        VOICE_LABEL,
        "{boss}",
        "{day}",
        "{time}",
    ]
    .into_iter()
    .map(|text| Protected::Exact(text.to_owned()))
    .collect()
}

/// Every code-owned text a rewrite request carries, for scanner exemptions.
pub fn code_owned_texts() -> Vec<String> {
    [
        NUDGE_REWRITE_INSTRUCTION,
        PLAYFUL_MOOD,
        GENTLE_MOOD,
        VOICE_LABEL,
    ]
    .map(str::to_owned)
    .to_vec()
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
        // The guard refuses an external route while pseudonymisation is off,
        // an operator setting like the governor's own `ExternalForbidden`.
        let roster = self.roster().ok_or(RewriteFailure::Misconfigured)?;
        let mut identity = open_session(self.codec.as_ref(), &route, &roster)
            .map_err(|_| RewriteFailure::Misconfigured)?;
        let owned = protected();
        let messages = vec![
            Message::System {
                content: encode_protected(identity.as_mut(), prompt.system(), &owned),
            },
            Message::User {
                content: encode_protected(identity.as_mut(), prompt.seed(), &owned),
            },
        ];
        let mut session = self
            .client
            .open_rewrite("nudge", deadline)
            .map_err(|error| classify(&error))?
            .with_scanner(identity.scanner());
        let request = ChatRequest {
            model: route.alias.clone(),
            messages,
            tools: Vec::new(),
            output_schema: None,
            max_output_tokens: REWRITE_MAX_OUTPUT_TOKENS,
            reasoning: None,
            sampling: None,
        };
        let response = session.complete(&request).await.map_err(|error| {
            if let SessionFailure::IdentityLeakBlocked(blocked) = &error.failure {
                eprintln!("{}", blocked.log_line());
            }
            classify(&error)
        })?;
        match response.content {
            Some(text) if !text.trim().is_empty() => identity
                .decode_reply(&text)
                .map_err(|_| RewriteFailure::Refused),
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
