use std::{collections::BTreeMap, fmt, time::Duration};

use hyper::header::HeaderValue;
use rustls::pki_types::CertificateDer;

use super::super::ModelCapabilities;

const MAX_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_BODY_BYTES: usize = 1_048_576;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpConfigError {
    InvalidBaseUrl,
    /// Plain `http` to a host other than loopback, `localhost` or `host.docker.internal`.
    InsecureHttp,
    InvalidBearerKey,
    /// `runtime::tls::install_ring_provider` has not run.
    CryptoProviderMissing,
    InvalidTrustRoots,
    InvalidTimeout,
    InvalidLimits,
}

impl fmt::Display for HttpConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidBaseUrl => "model endpoint URL is invalid",
            Self::InsecureHttp => "plain http is allowed only for loopback model endpoints",
            Self::InvalidBearerKey => "model endpoint key is empty or not header-safe",
            Self::CryptoProviderMissing => "no process TLS crypto provider is installed",
            Self::InvalidTrustRoots => "model endpoint trust roots are invalid",
            Self::InvalidTimeout => "model endpoint timeout is out of range",
            Self::InvalidLimits => "model endpoint body limits are out of range",
        })
    }
}

impl std::error::Error for HttpConfigError {}

/// Bearer key bytes loaded by the caller (from a file, never an env value).
pub struct BearerKey(Box<[u8]>);

impl BearerKey {
    /// Surrounding ASCII whitespace is trimmed; the rest must be printable ASCII.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, HttpConfigError> {
        let key = bytes.trim_ascii();
        if key.is_empty() || !key.iter().all(|byte| (0x21..=0x7e).contains(byte)) {
            return Err(HttpConfigError::InvalidBearerKey);
        }
        Ok(Self(key.into()))
    }

    pub(crate) fn header(&self) -> Option<HeaderValue> {
        let mut value = Vec::with_capacity(7 + self.0.len());
        value.extend_from_slice(b"Bearer ");
        value.extend_from_slice(&self.0);
        let mut header = HeaderValue::from_bytes(&value).ok()?;
        header.set_sensitive(true);
        Some(header)
    }
}

impl fmt::Debug for BearerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BearerKey(<redacted>)")
    }
}

/// Server certificate trust for `https` endpoints; verification is always on.
#[derive(Clone)]
pub enum TrustRoots {
    /// The compiled Mozilla root set (`webpki-roots`).
    WebPki,
    /// Replace the root set, e.g. with a private CA.
    Custom(Vec<CertificateDer<'static>>),
}

impl fmt::Debug for TrustRoots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WebPki => f.write_str("WebPki"),
            Self::Custom(roots) => f.debug_tuple("Custom").field(&roots.len()).finish(),
        }
    }
}

/// Wire body caps. Completions allow twice the runner's 512 KiB aggregate for JSON
/// escaping and envelope; listings get the 256 KiB payload bound; error bodies the
/// 64 KiB metadata bound. The runner still enforces its semantic limits.
#[derive(Clone, Debug)]
pub struct HttpLimits {
    pub completion_body_bytes: usize,
    pub models_body_bytes: usize,
    pub error_body_bytes: usize,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            completion_body_bytes: MAX_BODY_BYTES,
            models_body_bytes: 256 * 1024,
            error_body_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug)]
pub struct HttpProviderConfig {
    /// `scheme://host[:port][/prefix]`; one trailing `/v1` is accepted and trimmed.
    pub base_url: String,
    pub bearer_key: Option<BearerKey>,
    /// Per HTTP exchange: connect, TLS, request and the whole body.
    pub timeout: Duration,
    /// Operator-declared capabilities, used when the listing publishes none.
    pub declared: BTreeMap<String, ModelCapabilities>,
    pub trust_roots: TrustRoots,
    pub limits: HttpLimits,
    /// How long a successful `GET /models` listing is trusted.
    pub catalog_ttl: Duration,
    /// Upper bound for one listing fetch, including waiting on a concurrent one;
    /// the runner's remaining deadline may shorten it further.
    pub catalog_timeout: Duration,
    /// How long a failed or timed-out listing suppresses refetching.
    pub catalog_failure_ttl: Duration,
}

impl HttpProviderConfig {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            bearer_key: None,
            timeout: Duration::from_secs(120),
            declared: BTreeMap::new(),
            trust_roots: TrustRoots::WebPki,
            limits: HttpLimits::default(),
            catalog_ttl: Duration::from_secs(300),
            catalog_timeout: Duration::from_secs(5),
            catalog_failure_ttl: Duration::from_secs(30),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), HttpConfigError> {
        if self.timeout.is_zero()
            || self.timeout > MAX_TIMEOUT
            || self.catalog_ttl > MAX_TIMEOUT
            || self.catalog_timeout.is_zero()
            || self.catalog_timeout > MAX_TIMEOUT
            || self.catalog_failure_ttl > MAX_TIMEOUT
        {
            return Err(HttpConfigError::InvalidTimeout);
        }
        let limits = [
            self.limits.completion_body_bytes,
            self.limits.models_body_bytes,
            self.limits.error_body_bytes,
        ];
        if limits
            .iter()
            .any(|limit| *limit == 0 || *limit > MAX_BODY_BYTES)
        {
            return Err(HttpConfigError::InvalidLimits);
        }
        Ok(())
    }
}
