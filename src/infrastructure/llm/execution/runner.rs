use std::sync::Arc;

use super::super::{
    ChatRequest, CompletionResponse, ErrorCode, LlmError, LlmProvider, ModelCapabilities,
    ProviderFailure, ProviderFailureKind, shaping,
};
use super::{
    accounting::estimate,
    policy::{ExecutionLimits, RetryPolicy},
    request::validate_request,
    response::validate_response,
};

pub struct CompletionRunner<P> {
    provider: Arc<P>,
    limits: ExecutionLimits,
    retry: RetryPolicy,
}

impl<P: LlmProvider> CompletionRunner<P> {
    pub fn new(
        provider: Arc<P>,
        limits: ExecutionLimits,
        retry: RetryPolicy,
    ) -> Result<Self, LlmError> {
        limits.validate()?;
        retry.validate()?;
        Ok(Self {
            provider,
            limits,
            retry,
        })
    }

    pub async fn complete(&self, request: &ChatRequest) -> Result<CompletionResponse, LlmError> {
        let original_bytes = validate_request(request, &self.limits)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(self.retry.total_deadline)
            .ok_or_else(|| LlmError::new(ErrorCode::RequestInvalid, "deadline-overflow"))?;
        let wait = remaining_time(deadline)?;
        let mut capabilities =
            tokio::time::timeout(wait, self.provider.capabilities(&request.model, deadline))
                .await
                .map_err(|_| LlmError::new(ErrorCode::DeadlineExceeded, "deadline"))?;
        let (mut shaped, mut request_bytes) =
            self.shape(request, original_bytes, capabilities.as_ref())?;
        let mut downgraded = false;
        let mut remaining = self.limits.token_budget;
        let mut attempt = 1u8;
        loop {
            let current = shaped.as_ref().unwrap_or(request);
            let reservation = estimate(request_bytes, current.max_output_tokens)?;
            if reservation > remaining {
                return Err(LlmError::new(
                    ErrorCode::BudgetExceeded,
                    "attempt-reservation",
                ));
            }
            remaining -= reservation;
            let wait = remaining_time(deadline)?;
            let call = match &capabilities {
                Some(capabilities) => self.provider.complete_with(current, capabilities),
                None => self.provider.complete(current),
            };
            let outcome = tokio::time::timeout(wait, call)
                .await
                .map_err(|_| LlmError::new(ErrorCode::DeadlineExceeded, "deadline"))?;
            match outcome {
                Ok(response) => {
                    let known = response
                        .usage
                        .as_ref()
                        .map(|usage| {
                            usage.total().ok_or_else(|| {
                                LlmError::new(ErrorCode::BudgetExceeded, "usage-overflow")
                            })
                        })
                        .transpose()?;
                    if let Some(used) = known
                        && used > reservation
                    {
                        return Err(LlmError::new(
                            ErrorCode::BudgetExceeded,
                            "usage-reservation",
                        ));
                    }
                    return validate_response(current, response, &self.limits);
                }
                // One reshaped retry outside the transient attempt count; the
                // rejected attempt keeps its reservation.
                Err(ProviderFailure {
                    kind: ProviderFailureKind::CapabilityRejected(capability),
                    ..
                }) if !downgraded && capabilities.is_some() => {
                    downgraded = true;
                    let reduced = capabilities
                        .take()
                        .map(|capabilities| capabilities.without(capability));
                    (shaped, request_bytes) =
                        self.shape(request, original_bytes, reduced.as_ref())?;
                    capabilities = reduced;
                }
                Err(failure)
                    if failure.kind == ProviderFailureKind::Transient
                        && attempt < self.retry.max_attempts =>
                {
                    attempt += 1;
                    let wait = remaining_time(deadline)?;
                    if self.retry.backoff >= wait {
                        return Err(LlmError::new(ErrorCode::DeadlineExceeded, "backoff"));
                    }
                    tokio::time::sleep(self.retry.backoff).await;
                }
                Err(failure) => return Err(provider_error(failure.kind, failure.reason_code)),
            }
        }
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

fn remaining_time(deadline: tokio::time::Instant) -> Result<std::time::Duration, LlmError> {
    deadline
        .checked_duration_since(tokio::time::Instant::now())
        .ok_or_else(|| LlmError::new(ErrorCode::DeadlineExceeded, "deadline"))
}

fn provider_error(kind: ProviderFailureKind, reason: &str) -> LlmError {
    match kind {
        ProviderFailureKind::Authentication => {
            LlmError::new(ErrorCode::ProviderAuthentication, reason)
        }
        ProviderFailureKind::InvalidOutput => LlmError::new(ErrorCode::InvalidOutput, reason),
        ProviderFailureKind::Transient
        | ProviderFailureKind::Permanent
        | ProviderFailureKind::CapabilityRejected(_) => {
            LlmError::new(ErrorCode::ProviderPermanent, reason)
        }
    }
}
