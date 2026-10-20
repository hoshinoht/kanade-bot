use std::fmt;

crate::domain::spelled! {
    /// `as_str`: a stable snake-case name for logs and transcripts.
    ErrorCode {
        RequestInvalid => "request_invalid",
        BudgetExceeded => "budget_exceeded",
        DeadlineExceeded => "deadline_exceeded",
        ProviderPermanent => "provider_permanent",
        ProviderAuthentication => "provider_authentication",
        InvalidOutput => "invalid_output",
        ModelMismatch => "model_mismatch",
        /// Cut off by length or an unknown finish reason.
        Incomplete => "incomplete",
        /// The provider's content filter blocked the reply (user decision: kept apart
        /// from a length cut-off). Permanent for that request; the backend is healthy.
        ContentFiltered => "content_filtered",
        /// The model's capabilities cannot honour a requested feature; nothing was sent.
        UnsupportedCapability => "unsupported_capability",
        /// Gateway admission turned the request away (no work ran).
        AdmissionRefused => "admission_refused",
        BackendUnavailable => "backend_unavailable",
        /// Upstream or transport timeout; the backend may still be working.
        UpstreamTimeout => "upstream_timeout",
        /// The gateway key has expired (Kanata `key_expired`); rotate it.
        KeyExpired => "key_expired",
        /// The provider account is out of quota or credit (OpenAI's
        /// `429 insufficient_quota`); an operator must top it up.
        QuotaExhausted => "quota_exhausted",
    }
}

#[derive(Clone, PartialEq, Eq, thiserror::Error)]
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

fn digest(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}
