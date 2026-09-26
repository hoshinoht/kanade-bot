//! One tiny completion per role for `kanade models check --probe`: a fixed
//! prompt with no member data, through the same governed session kind and
//! route guard as the role's real calls.

use std::time::Duration;

use tokio::time::Instant;

use super::super::{
    ChatRequest, Effort, FinishReason, Message,
    governor::{QuestionLimits, Refused, Role, SessionError, SessionFailure},
    identity::{IdentityCodec, LeakScanner, open_session},
};
use super::ModelStack;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_MAX_TOKENS: u32 = 128;
const WHO: &str = "models-check";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub role: Role,
    pub alias: String,
    pub effort: Effort,
    pub outcome: ProbeOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok {
        latency_ms: u64,
        finish_reason: String,
    },
    /// Nothing was sent (route guard or governor refusal).
    Refused(String),
    Failed(String),
}

impl ProbeOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }
}

impl ModelStack {
    /// `None` when the role has no alias.
    pub async fn probe(
        &self,
        role: Role,
        codec: &dyn IdentityCodec,
        timeout: Duration,
    ) -> Option<ProbeResult> {
        let route = self.governor.route(role)?;
        let effort = self.effort(role).unwrap_or(Effort::Off);
        let result = |outcome| ProbeResult {
            role,
            alias: route.alias.clone(),
            effort,
            outcome,
        };
        // The fixed prompt carries no member data: an empty-roster grant.
        let scanner = match open_session(codec, &route, &[]) {
            Ok(identity) => identity.scanner(),
            Err(refused) => return Some(result(ProbeOutcome::Refused(refused.to_string()))),
        };
        let request = ChatRequest {
            model: route.alias.clone(),
            messages: vec![
                Message::System {
                    content: "This is a connectivity check. Reply with the single word: ok".into(),
                },
                Message::User {
                    content: "ping".into(),
                },
            ],
            tools: Vec::new(),
            output_schema: None,
            max_output_tokens: PROBE_MAX_TOKENS,
            reasoning: Some(effort),
            sampling: None,
        };
        let mut outcome = self.attempt(role, &request, &scanner, timeout).await;
        // Rewrites never wait for a rate token; the probes before this one may
        // have just emptied the bucket, so wait out the refill once.
        if let (Role::Rewrite, Err(Refusal::Rate(wait))) = (role, &outcome)
            && *wait < timeout
        {
            tokio::time::sleep(*wait).await;
            outcome = self.attempt(role, &request, &scanner, timeout).await;
        }
        Some(result(match outcome {
            Ok(outcome) => outcome,
            Err(Refusal::Rate(_)) => ProbeOutcome::Refused(
                Refused::RateLimited {
                    wait: Duration::ZERO,
                }
                .to_string(),
            ),
            Err(Refusal::Other(reason)) => ProbeOutcome::Refused(reason),
        }))
    }

    async fn attempt(
        &self,
        role: Role,
        request: &ChatRequest,
        scanner: &LeakScanner,
        timeout: Duration,
    ) -> Result<ProbeOutcome, Refusal> {
        let client = &self.client;
        let session = match role {
            Role::Extraction => client.open_extraction(WHO, timeout, timeout).await,
            Role::Chat => {
                let limits = QuestionLimits {
                    tool_rounds: 1,
                    timeout,
                };
                client.open_question(WHO, true, limits).await
            }
            Role::Rewrite => client.open_rewrite(WHO, timeout),
        };
        let mut session = session
            .map_err(Refusal::from)?
            .with_scanner(scanner.clone());
        let started = Instant::now();
        match session.complete(request).await {
            Ok(response) => Ok(ProbeOutcome::Ok {
                latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                finish_reason: finish(&response.finish_reason),
            }),
            Err(
                error @ SessionError {
                    failure: SessionFailure::Refused(_),
                    ..
                },
            ) => Err(Refusal::from(error)),
            Err(error) => Ok(ProbeOutcome::Failed(error.to_string())),
        }
    }
}

/// Nothing was sent.
enum Refusal {
    Rate(Duration),
    Other(String),
}

impl From<SessionError> for Refusal {
    fn from(error: SessionError) -> Self {
        match error.failure {
            SessionFailure::Refused(Refused::RateLimited { wait }) => Self::Rate(wait),
            _ => Self::Other(error.to_string()),
        }
    }
}

fn finish(reason: &FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop".into(),
        FinishReason::ToolCalls => "tool_calls".into(),
        FinishReason::Length => "length".into(),
        FinishReason::ContentFilter => "content_filter".into(),
        FinishReason::Other(other) => other.clone(),
    }
}
