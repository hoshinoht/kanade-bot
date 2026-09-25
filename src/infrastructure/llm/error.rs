use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    RequestInvalid,
    BudgetExceeded,
    DeadlineExceeded,
    ProviderPermanent,
    ProviderAuthentication,
    InvalidOutput,
    ModelMismatch,
    /// Cut off by length or an unknown finish reason.
    Incomplete,
    /// The provider's content filter blocked the reply (user decision: kept apart
    /// from a length cut-off). Permanent for that request; the backend is healthy.
    ContentFiltered,
    /// The model's capabilities cannot honour a requested feature; nothing was sent.
    UnsupportedCapability,
    /// Gateway admission turned the request away (no work ran).
    AdmissionRefused,
    BackendUnavailable,
    /// Upstream or transport timeout; the backend may still be working.
    UpstreamTimeout,
    /// The gateway key has expired (Kanata `key_expired`); rotate it.
    KeyExpired,
}

#[derive(Clone, PartialEq, Eq)]
pub struct LlmError {
    pub code: ErrorCode,
    pub digest: u64,
}

impl LlmError {
    pub(crate) fn new(code: ErrorCode, safe_reason: &str) -> Self {
        Self {
            code,
            digest: digest(safe_reason),
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "LLM completion failed ({:?}, digest={:016x})",
            self.code, self.digest
        )
    }
}

impl fmt::Debug for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmError")
            .field("code", &self.code)
            .field("digest", &format_args!("{:016x}", self.digest))
            .finish()
    }
}

impl std::error::Error for LlmError {}

fn digest(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
