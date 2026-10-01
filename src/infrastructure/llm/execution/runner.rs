use std::{sync::Arc, time::Duration};

use tokio::time::Instant;

use super::super::{
    ChatRequest, CompletionResponse, ErrorCode, LlmError, LlmProvider, ModelCapabilities,
    ProviderFailureKind,
    governor::{CallKind, Outcome, Random, SentRequest, full_jitter},
    http::KEY_EXPIRED,
    shaping,
    wire::sent_effort,
};
use super::{
    accounting::estimate,
    gate::{Denied, Gate},
    policy::{ExecutionLimits, MAX_BACKOFF, RetryPolicy},
    request::validate_request,
    response::validate_response,
};

/// Runs one completion through a [`Gate`]. Only governor sessions construct and
/// drive it; test support adds an ungoverned constructor.
pub struct CompletionRunner<P> {
    provider: Arc<P>,
    limits: ExecutionLimits,
    retry: RetryPolicy,
    random: Arc<dyn Random>,
}

pub(in crate::infrastructure::llm) enum RunError {
    /// The gate refused the first request; nothing was sent.
    Denied(Denied),
    Failed(RunFailure),
}

pub(in crate::infrastructure::llm) struct RunFailure {
    pub(in crate::infrastructure::llm) error: LlmError,
    pub(in crate::infrastructure::llm) cause: Cause,
    /// The backend may have worked for this call (a reply arrived or a request timed out).
    pub(in crate::infrastructure::llm) charged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::infrastructure::llm) enum Cause {
    Admission { retry_after: Option<Duration> },
    Timeout,
    Other,
}

fn failed(error: LlmError, charged: bool) -> RunError {
    RunError::Failed(RunFailure {
        error,
        cause: Cause::Other,
        charged,
    })
}

impl<P: LlmProvider> CompletionRunner<P> {
    pub(in crate::infrastructure::llm) fn new(
        provider: Arc<P>,
        limits: ExecutionLimits,
        retry: RetryPolicy,
        random: Arc<dyn Random>,
    ) -> Result<Self, LlmError> {
        limits.validate()?;
        retry.validate()?;
        Ok(Self {
            provider,
            limits,
            retry,
            random,
        })
    }

    pub(in crate::infrastructure::llm) fn random(&self) -> &Arc<dyn Random> {
        &self.random
    }

    /// Test support only: no governor, and backoff draws the full-jitter upper bound.
    #[cfg(any(test, feature = "test-support"))]
    pub fn ungoverned(
        provider: Arc<P>,
        limits: ExecutionLimits,
        retry: RetryPolicy,
    ) -> Result<Self, LlmError> {
        Self::new(provider, limits, retry, Arc::new(UpperBound))
    }

    /// Test support only: one ungoverned call with non-chat retry rules.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn complete(&self, request: &ChatRequest) -> Result<CompletionResponse, LlmError> {
        self.complete_as(request, CallKind::Extraction).await
    }

    /// Test support only: one ungoverned call with `kind`'s retry and
    /// tool-call validation rules.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn complete_as(
        &self,
        request: &ChatRequest,
        kind: CallKind,
    ) -> Result<CompletionResponse, LlmError> {
        let random = self.random.clone();
        let mut used = 0;
        let mut gate = Gate::ungoverned(random.as_ref(), &mut used, kind);
        self.run(request, &mut gate)
            .await
            .map_err(|error| match error {
                RunError::Failed(failure) => failure.error,
                RunError::Denied(_) => LlmError::new(ErrorCode::BudgetExceeded, "gate"),
            })
    }

    pub(in crate::infrastructure::llm) async fn run(
        &self,
        request: &ChatRequest,
        gate: &mut Gate<'_>,
    ) -> Result<CompletionResponse, RunError> {
        let local = |error| failed(error, false);
        let original_bytes = validate_request(request, &self.limits).map_err(local)?;
        let own = Instant::now()
            .checked_add(self.retry.total_deadline)
            .ok_or_else(|| {
                local(LlmError::new(
                    ErrorCode::RequestInvalid,
                    "deadline-overflow",
                ))
            })?;
        let deadline = gate.deadline().map_or(own, |outer| outer.min(own));
        let wait = remaining_time(deadline).map_err(local)?;
        let mut capabilities =
            tokio::time::timeout(wait, self.provider.capabilities(&request.model, deadline))
                .await
                .map_err(|_| local(LlmError::new(ErrorCode::DeadlineExceeded, "deadline")))?;
        let (mut shaped, mut request_bytes) = self
            .shape(request, original_bytes, capabilities.as_ref())
            .map_err(local)?;
        let mut downgraded = false;
        let mut remaining = self.limits.token_budget;
        let mut attempt = 1u8;
        let mut charged = false;
        let mut retry = false;
        // The failure a refused retry falls back to.
        let mut last: Option<RunFailure> = None;
        loop {
            let current = shaped.as_ref().unwrap_or(request);
            let reservation = estimate(request_bytes, current.max_output_tokens)
                .map_err(|error| failed(error, charged))?;
            if reservation > remaining {
                return Err(failed(
                    LlmError::new(ErrorCode::BudgetExceeded, "attempt-reservation"),
                    charged,
                ));
            }
            if let Err(denied) = gate.admit(retry, deadline).await {
                return Err(match last {
                    Some(failure) => RunError::Failed(failure),
                    None => RunError::Denied(denied),
                });
            }
            retry = false;
            remaining -= reservation;
            // The alias and the effort as the wire body carries them (the
            // same rule `chat_body` applies); unknown capabilities send the
            // request's effort as is.
            gate.note_sent(SentRequest {
                alias: current.model.clone(),
                effort: match &capabilities {
                    Some(capabilities) => sent_effort(current, capabilities),
                    None => current.reasoning,
                },
            });
            let Ok(wait) = remaining_time(deadline) else {
                gate.finish(None);
                return Err(failed(
                    LlmError::new(ErrorCode::DeadlineExceeded, "deadline"),
                    charged,
                ));
            };
            let request_id = gate.request_id();
            let call = match (&capabilities, &request_id) {
                (Some(capabilities), Some(id)) => {
                    self.provider.complete_tagged(current, capabilities, id)
                }
                (Some(capabilities), None) => self.provider.complete_with(current, capabilities),
                (None, _) => self.provider.complete(current),
            };
            let Ok(outcome) = tokio::time::timeout(wait, call).await else {
                // Our own deadline cut the request off: the backend may still be
                // working, but its health is unknown.
                gate.finish(None);
                return Err(RunError::Failed(RunFailure {
                    error: LlmError::new(ErrorCode::DeadlineExceeded, "deadline"),
                    cause: Cause::Timeout,
                    charged: true,
                }));
            };
            let failure = match outcome {
                Ok(response) => {
                    gate.finish(Some(Outcome::Success));
                    let charge = |error| failed(error, true);
                    let known = response
                        .usage
                        .as_ref()
                        .map(|usage| {
                            usage.total().ok_or_else(|| {
                                LlmError::new(ErrorCode::BudgetExceeded, "usage-overflow")
                            })
                        })
                        .transpose()
                        .map_err(charge)?;
                    if let Some(used) = known
                        && used > reservation
                    {
                        return Err(charge(LlmError::new(
                            ErrorCode::BudgetExceeded,
                            "usage-reservation",
                        )));
                    }
                    let validation = gate.kind().tool_call_validation();
                    let valid = validate_response(current, response, &self.limits, validation)
                        .map_err(charge)?;
                    return Ok(valid);
                }
                Err(failure) => failure,
            };
            gate.finish(Some(outcome_of(failure.kind)));
            charged |= matches!(
                failure.kind,
                ProviderFailureKind::UpstreamTimeout | ProviderFailureKind::InvalidOutput
            );
            let this = RunFailure {
                error: provider_error(failure.kind, failure.reason_code),
                cause: cause_of(failure.kind),
                charged,
            };
            match failure.kind {
                // One reshaped retry outside the transient attempt count; the
                // rejected attempt keeps its reservation.
                ProviderFailureKind::CapabilityRejected(capability)
                    if !downgraded && capabilities.is_some() =>
                {
                    downgraded = true;
                    let reduced = capabilities
                        .take()
                        .map(|capabilities| capabilities.without(capability));
                    (shaped, request_bytes) = self
                        .shape(request, original_bytes, reduced.as_ref())
                        .map_err(|error| failed(error, charged))?;
                    capabilities = reduced;
                    last = Some(this);
                }
                kind if retryable(kind, gate.kind()) && attempt < self.retry.max_attempts => {
                    if gate.check_retry().is_err() {
                        return Err(RunError::Failed(this));
                    }
                    let mut pause = full_jitter(gate.random(), self.backoff_cap(attempt));
                    if let ProviderFailureKind::RateLimited { retry_after } = kind {
                        pause = pause.max(retry_after);
                    }
                    attempt += 1;
                    let wait = remaining_time(deadline).map_err(|error| failed(error, charged))?;
                    if pause >= wait {
                        return Err(failed(
                            LlmError::new(ErrorCode::DeadlineExceeded, "backoff"),
                            charged,
                        ));
                    }
                    tokio::time::sleep(pause).await;
                    retry = true;
                    last = Some(this);
                }
                _ => return Err(RunError::Failed(this)),
            }
        }
    }

    /// Exponential cap for the `attempt`-th retry; the pause is drawn below it.
    fn backoff_cap(&self, attempt: u8) -> Duration {
        let factor = 1u32 << u32::from(attempt.saturating_sub(1)).min(16);
        self.retry
            .backoff
            .checked_mul(factor)
            .map_or(MAX_BACKOFF, |cap| cap.min(MAX_BACKOFF))
    }

    /// Shaping may add a schema instruction, which must count toward every bound.
    fn shape(
        &self,
        request: &ChatRequest,
        original_bytes: usize,
        capabilities: Option<&ModelCapabilities>,
    ) -> Result<(Option<ChatRequest>, usize), LlmError> {
        let shaped = match capabilities {
            Some(capabilities) => shaping::prepare(request, capabilities)?,
            None => None,
        };
        let bytes = match &shaped {
            Some(shaped) => validate_request(shaped, &self.limits)?,
            None => original_bytes,
        };
        Ok((shaped, bytes))
    }
}

fn remaining_time(deadline: Instant) -> Result<Duration, LlmError> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| LlmError::new(ErrorCode::DeadlineExceeded, "deadline"))
}

/// Admission refusals are only ever requeued by the session, never retried here;
/// a timed-out chat request may still be running upstream.
fn retryable(kind: ProviderFailureKind, call: CallKind) -> bool {
    match kind {
        ProviderFailureKind::Transient | ProviderFailureKind::RateLimited { .. } => true,
        ProviderFailureKind::UpstreamTimeout => call != CallKind::Chat,
        _ => false,
    }
}

fn outcome_of(kind: ProviderFailureKind) -> Outcome {
    match kind {
        ProviderFailureKind::Transient | ProviderFailureKind::RateLimited { .. } => {
            Outcome::TransientFailure
        }
        ProviderFailureKind::UpstreamTimeout => Outcome::Timeout,
        ProviderFailureKind::BackendUnavailable { retry_after: None } => {
            Outcome::BackendUnavailable
        }
        ProviderFailureKind::BackendUnavailable {
            retry_after: Some(retry_after),
        } => Outcome::BackendUnavailableFor(retry_after),
        ProviderFailureKind::AdmissionRefused { .. } => Outcome::AdmissionRefused,
        ProviderFailureKind::Permanent
        | ProviderFailureKind::Authentication
        | ProviderFailureKind::InvalidOutput
        | ProviderFailureKind::CapabilityRejected(_) => Outcome::Rejected,
    }
}

fn cause_of(kind: ProviderFailureKind) -> Cause {
    match kind {
        ProviderFailureKind::AdmissionRefused { retry_after } => Cause::Admission { retry_after },
        ProviderFailureKind::UpstreamTimeout => Cause::Timeout,
        _ => Cause::Other,
    }
}

fn provider_error(kind: ProviderFailureKind, reason: &str) -> LlmError {
    let code = match kind {
        // A Kanata route size rejection is a caller-side configuration/budget
        // fault, never an unsupported optional capability.
        ProviderFailureKind::Permanent if reason == "size-limit" => ErrorCode::RequestInvalid,
        ProviderFailureKind::Authentication if reason == KEY_EXPIRED => ErrorCode::KeyExpired,
        ProviderFailureKind::Authentication => ErrorCode::ProviderAuthentication,
        ProviderFailureKind::InvalidOutput => ErrorCode::InvalidOutput,
        ProviderFailureKind::AdmissionRefused { .. } => ErrorCode::AdmissionRefused,
        ProviderFailureKind::BackendUnavailable { .. } => ErrorCode::BackendUnavailable,
        ProviderFailureKind::UpstreamTimeout => ErrorCode::UpstreamTimeout,
        ProviderFailureKind::Transient
        | ProviderFailureKind::RateLimited { .. }
        | ProviderFailureKind::Permanent
        | ProviderFailureKind::CapabilityRejected(_) => ErrorCode::ProviderPermanent,
    };
    LlmError::new(code, reason)
}

/// Makes ungoverned backoff deterministic: every draw is the top of its range.
#[cfg(any(test, feature = "test-support"))]
struct UpperBound;

#[cfg(any(test, feature = "test-support"))]
impl Random for UpperBound {
    fn next_u64(&self) -> u64 {
        u64::MAX
    }
}
