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

impl ErrorCode {
    /// A stable snake-case name for logs and transcripts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequestInvalid => "request_invalid",
            Self::BudgetExceeded => "budget_exceeded",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ProviderPermanent => "provider_permanent",
            Self::ProviderAuthentication => "provider_authentication",
            Self::InvalidOutput => "invalid_output",
            Self::ModelMismatch => "model_mismatch",
            Self::Incomplete => "incomplete",
            Self::ContentFiltered => "content_filtered",
            Self::UnsupportedCapability => "unsupported_capability",
            Self::AdmissionRefused => "admission_refused",
            Self::BackendUnavailable => "backend_unavailable",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::KeyExpired => "key_expired",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct LlmError {
    pub code: ErrorCode,
    pub digest: u64,
    /// What an `Incomplete` reply reported, so the log can say why it stopped.
    cutoff: Option<Cutoff>,
}

/// The counts a reply rejected as `Incomplete` carried; never its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cutoff {
    /// `finish=length`; otherwise an unknown finish reason, whose provider
    /// text is not kept.
    pub length: bool,
    /// The `max_tokens` sent; `None` when the body carried none.
    pub max_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub reasoning_tokens: Option<u64>,
}

impl LlmError {
    pub(crate) fn new(code: ErrorCode, safe_reason: &str) -> Self {
        Self {
            code,
            digest: digest(safe_reason),
            cutoff: None,
        }
    }

    /// A reply that ended before finishing (`ErrorCode::Incomplete`).
    pub(crate) fn incomplete(cutoff: Cutoff) -> Self {
        Self {
            cutoff: Some(cutoff),
            ..Self::new(ErrorCode::Incomplete, "finish")
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(cutoff) = &self.cutoff else {
            return write!(
                formatter,
                "LLM completion failed ({:?}, digest={:016x})",
                self.code, self.digest
            );
        };
        if cutoff.length {
            formatter.write_str("Incomplete: reply cut off at the token limit (finish=length, ")?;
        } else {
            formatter.write_str("Incomplete: reply ended without finishing (finish=other, ")?;
        }
        match (cutoff.completion_tokens, cutoff.max_tokens) {
            (Some(used), Some(limit)) => write!(formatter, "{used} of {limit} tokens")?,
            (Some(used), None) => write!(formatter, "{used} tokens")?,
            (None, Some(limit)) => write!(formatter, "usage not reported, limit {limit} tokens")?,
            (None, None) => formatter.write_str("usage not reported")?,
        }
        if let Some(reasoning) = cutoff.reasoning_tokens {
            write!(formatter, ", {reasoning} reasoning")?;
        }
        formatter.write_str(")")
    }
}

impl fmt::Debug for LlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmError")
            .field("code", &self.code)
            .field("digest", &format_args!("{:016x}", self.digest))
            .field("cutoff", &self.cutoff)
            .finish()
    }
}

impl std::error::Error for LlmError {}

fn digest(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
