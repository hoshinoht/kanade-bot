use std::sync::Arc;

use super::super::{
    ChatRequest, CompletionResponse, ErrorCode, LlmError, LlmProvider, ProviderFailureKind,
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
        let request_bytes = validate_request(request, &self.limits)?;
        let reservation = estimate(request_bytes, request.max_output_tokens)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(self.retry.total_deadline)
            .ok_or_else(|| LlmError::new(ErrorCode::RequestInvalid, "deadline-overflow"))?;
        let mut remaining = self.limits.token_budget;
        for attempt in 1..=self.retry.max_attempts {
            if reservation > remaining {
                return Err(LlmError::new(
                    ErrorCode::BudgetExceeded,
                    "attempt-reservation",
                ));
            }
            remaining -= reservation;
            let wait = deadline
                .checked_duration_since(tokio::time::Instant::now())
                .ok_or_else(|| LlmError::new(ErrorCode::DeadlineExceeded, "deadline"))?;
            let outcome = tokio::time::timeout(wait, self.provider.complete(request))
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
                    return validate_response(request, response, &self.limits);
                }
                Err(failure)
                    if failure.kind == ProviderFailureKind::Transient
                        && attempt < self.retry.max_attempts =>
                {
                    let wait = deadline
                        .checked_duration_since(tokio::time::Instant::now())
                        .ok_or_else(|| LlmError::new(ErrorCode::DeadlineExceeded, "deadline"))?;
                    if self.retry.backoff >= wait {
                        return Err(LlmError::new(ErrorCode::DeadlineExceeded, "backoff"));
                    }
                    tokio::time::sleep(self.retry.backoff).await;
                }
                Err(failure) => return Err(provider_error(failure.kind, failure.reason_code)),
            }
        }
        Err(LlmError::new(
            ErrorCode::ProviderPermanent,
            "retry-exhausted",
        ))
    }
}

fn provider_error(kind: ProviderFailureKind, reason: &str) -> LlmError {
    match kind {
        ProviderFailureKind::Authentication => {
            LlmError::new(ErrorCode::ProviderAuthentication, reason)
        }
        ProviderFailureKind::InvalidOutput => LlmError::new(ErrorCode::InvalidOutput, reason),
        ProviderFailureKind::Transient | ProviderFailureKind::Permanent => {
            LlmError::new(ErrorCode::ProviderPermanent, reason)
        }
    }
}
