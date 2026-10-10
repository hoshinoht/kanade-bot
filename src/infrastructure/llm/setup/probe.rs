//! One tiny completion per role for `kanade models check --probe`: a fixed
//! prompt with no member data, through the same governed session kind and
//! route guard as the role's real calls.

use std::time::Duration;

use tokio::time::Instant;

use super::super::{
    ChatRequest, Effort, FinishReason, Message,
    governor::{QuestionLimits, Refused, Role, SessionError, SessionFailure},
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
    /// The `x-request-id`s the probe sent, to find it in the gateway log;
    /// empty when nothing tagged went out.
    pub request_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    Ok {
        latency_ms: u64,
        finish_reason: String,
    },
    /// Turned away by the governor; ids name any earlier tagged request of the
    /// session (a refused requeue), else none were sent.
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
    pub async fn probe(&self, role: Role, timeout: Duration) -> Option<ProbeResult> {
        let route = self.governor.route(role)?;
        let effort = self.effort(role).unwrap_or(Effort::Off);
        let result = |(outcome, request_ids)| ProbeResult {
            role,
            alias: route.alias.clone(),
            effort,
            outcome,
            request_ids,
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
        let mut outcome = self.attempt(&route, &request, timeout).await;
        // Rewrites never wait for a rate token; the probes before this one may
        // have just emptied the bucket, so wait out the refill once.
        if let (Role::Rewrite, Err((Refusal::Rate(wait), first))) = (role, &outcome)
            && *wait < timeout
        {
            let first = first.clone();
            tokio::time::sleep(*wait).await;
            outcome = self.attempt(&route, &request, timeout).await;
            // Ids the refused first try sent stay in the report.
            match &mut outcome {
                Ok((_, ids)) | Err((_, ids)) => ids.splice(0..0, first).for_each(drop),
            }
        }
        Some(result(match outcome {
            Ok(sent) => sent,
            // A refusal after a tagged requeue still names what went out.
            Err((Refusal::Rate(_), ids)) => (
                ProbeOutcome::Refused(
                    Refused::RateLimited {
                        wait: Duration::ZERO,
                    }
                    .to_string(),
                ),
                ids,
            ),
            Err((Refusal::Other(reason), ids)) => (ProbeOutcome::Refused(reason), ids),
        }))
    }

    async fn attempt(
        &self,
        route: &super::super::governor::RoleRoute,
        request: &ChatRequest,
        timeout: Duration,
    ) -> Result<(ProbeOutcome, Vec<String>), (Refusal, Vec<String>)> {
        let client = &self.client;
        let session = match route.role {
            Role::Extraction => {
                client
                    .open_extraction_on(route, WHO, timeout, timeout)
                    .await
            }
            Role::Chat => {
                let limits = QuestionLimits {
                    tool_rounds: 1,
                    timeout,
                };
                client.open_question_on(route, WHO, true, limits).await
            }
            Role::Rewrite => client.open_rewrite_on(route, WHO, timeout),
        };
        let mut session = session.map_err(|error| (Refusal::from(error), Vec::new()))?;
        let started = Instant::now();
        let sent = session.complete(request).await;
        let ids = session.request_ids().to_vec();
        match sent {
            Ok(response) => Ok((
                ProbeOutcome::Ok {
                    latency_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                    finish_reason: finish(&response.finish_reason),
                },
                ids,
            )),
            Err(
                error @ SessionError {
                    failure: SessionFailure::Refused(_),
                    ..
                },
            ) => Err((Refusal::from(error), ids)),
            Err(error) => Ok((ProbeOutcome::Failed(error.to_string()), ids)),
        }
    }
}

/// Turned away by the governor (nothing sent for this attempt, though an
/// earlier tagged request of the session may have been).
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
