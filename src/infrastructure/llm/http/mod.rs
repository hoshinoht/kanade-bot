mod config;
mod endpoint;
mod transport;

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use hyper::{Method, StatusCode};
use serde_json::Value;
use tokio::time::Instant;

pub use config::{BearerKey, HttpConfigError, HttpLimits, HttpProviderConfig, TrustRoots};

use super::{
    Capability, CapabilityFuture, ChatRequest, CompletionFuture, CompletionResponse, ListedModel,
    LlmProvider, ModelCapabilities, ProviderFailure, ProviderFailureKind,
    capabilities::{self, DowngradeCache, field_capability},
    parse_models_list, wire,
};
use endpoint::Endpoint;
use transport::{Reply, Transport};

type Models = Arc<BTreeMap<String, Option<ModelCapabilities>>>;
type ListingObserver = Box<dyn Fn(&[ListedModel]) + Send + Sync>;

#[derive(Default)]
struct Catalog {
    listing: Option<(Instant, Models)>,
    failed_at: Option<Instant>,
}

/// A cache decision: `Some(Some)` fresh listing, `Some(None)` recent failure, `None` fetch.
type Cached = Option<Option<Models>>;

/// OpenAI-compatible `chat/completions` and `models` client for Kanata or a direct
/// Ollama `/v1` endpoint. Request bodies follow each alias's effective capabilities.
pub struct OpenAiCompatibleProvider {
    transport: Transport,
    declared: BTreeMap<String, ModelCapabilities>,
    limits: HttpLimits,
    catalog_ttl: Duration,
    catalog_timeout: Duration,
    catalog_failure_ttl: Duration,
    catalog: Mutex<Catalog>,
    /// Single-flight guard so concurrent lookups share one listing fetch.
    refresh: tokio::sync::Mutex<()>,
    downgrades: DowngradeCache,
    observer: OnceLock<ListingObserver>,
}

enum PostError {
    Rejected(&'static str, Capability),
    Failed(ProviderFailure),
}

impl OpenAiCompatibleProvider {
    /// `https` endpoints require the process crypto provider from
    /// `runtime::tls::install_ring_provider`.
    pub fn new(config: HttpProviderConfig) -> Result<Self, HttpConfigError> {
        config.validate()?;
        let endpoint = Endpoint::parse(&config.base_url)?;
        Ok(Self {
            transport: Transport::new(
                endpoint,
                &config.trust_roots,
                config.bearer_key,
                config.timeout,
            )?,
            declared: config.declared,
            limits: config.limits,
            catalog_ttl: config.catalog_ttl,
            catalog_timeout: config.catalog_timeout,
            catalog_failure_ttl: config.catalog_failure_ttl,
            catalog: Mutex::new(Catalog::default()),
            refresh: tokio::sync::Mutex::new(()),
            downgrades: DowngradeCache::default(),
            observer: OnceLock::new(),
        })
    }

    /// Called after every successful listing, including the runner's own
    /// refreshes. Only one observer; returns false if one is already set.
    pub fn observe_listings(
        &self,
        observer: impl Fn(&[ListedModel]) + Send + Sync + 'static,
    ) -> bool {
        self.observer.set(Box::new(observer)).is_ok()
    }

    /// `GET {base}/models`; a successful listing refreshes the capability catalog.
    pub async fn list_models(&self) -> Result<Vec<ListedModel>, ProviderFailure> {
        let reply = self
            .transport
            .send(
                Method::GET,
                "models",
                None,
                None,
                self.limits.models_body_bytes,
                self.limits.error_body_bytes,
            )
            .await?;
        if reply.status != StatusCode::OK {
            return Err(status_failure(&reply));
        }
        let models = parse_models_list(&reply.body).ok_or(ProviderFailure {
            kind: ProviderFailureKind::InvalidOutput,
            reason_code: "malformed-models",
        })?;
        *self.lock_catalog() = Catalog {
            listing: Some((Instant::now(), catalog_of(&models))),
            failed_at: None,
        };
        if let Some(observer) = self.observer.get() {
            observer(&models);
        }
        Ok(models)
    }

    /// Published, else declared, else minimal capabilities, less any downgrades.
    pub async fn model_capabilities(&self, alias: &str) -> ModelCapabilities {
        self.resolve(alias, self.catalog_timeout).await
    }

    /// A fresh listing is authoritative for `catalog_ttl`; otherwise one fetch is
    /// shared by concurrent callers. Anything short of a listing falls back to
    /// declared or minimal capabilities.
    async fn resolve(&self, alias: &str, budget: Duration) -> ModelCapabilities {
        let models = match self.cached() {
            Some(models) => models,
            None => self.fetch(budget).await,
        };
        let published = models
            .as_ref()
            .and_then(|models| models.get(alias))
            .and_then(Option::as_ref);
        let resolved = capabilities::resolve(published, self.declared.get(alias));
        self.downgrades.apply(alias, resolved)
    }

    /// Waits for the single-flight lock within `budget`. The fetch itself gets the
    /// full `catalog_timeout` when `budget` allows it, else what is left of `budget`.
    /// Only a failed fetch or one that exceeded the full timeout suppresses
    /// refetching for `catalog_failure_ttl`; a caller's shorter budget or lock wait
    /// never does.
    async fn fetch(&self, budget: Duration) -> Option<Models> {
        let full = budget >= self.catalog_timeout;
        let deadline = Instant::now() + budget;
        let Ok(_flight) = tokio::time::timeout_at(deadline, self.refresh.lock()).await else {
            return None;
        };
        if let Some(models) = self.cached() {
            return models;
        }
        let limit = if full {
            self.catalog_timeout
        } else {
            deadline.saturating_duration_since(Instant::now())
        };
        match tokio::time::timeout(limit, self.list_models()).await {
            // Use this fetch directly: a zero TTL is already stale.
            Ok(Ok(listed)) => Some(catalog_of(&listed)),
            Ok(Err(_)) => {
                self.mark_failed();
                None
            }
            Err(_) => {
                if full {
                    self.mark_failed();
                }
                None
            }
        }
    }

    fn cached(&self) -> Cached {
        let catalog = self.lock_catalog();
        if let Some((fetched, models)) = &catalog.listing
            && fetched.elapsed() < self.catalog_ttl
        {
            return Some(Some(models.clone()));
        }
        catalog
            .failed_at
            .filter(|failed| failed.elapsed() < self.catalog_failure_ttl)
            .map(|_| None)
    }

    fn mark_failed(&self) {
        let mut catalog = self.lock_catalog();
        let fresh = catalog
            .listing
            .as_ref()
            .is_some_and(|(fetched, _)| fetched.elapsed() < self.catalog_ttl);
        if !fresh {
            catalog.failed_at = Some(Instant::now());
        }
    }

    /// The body is built from exactly `capabilities`. A 400 naming a sent optional
    /// field records the downgrade and returns `CapabilityRejected` so the runner
    /// can reshape, recount and retry once.
    async fn chat(
        &self,
        request: &ChatRequest,
        capabilities: &ModelCapabilities,
        request_id: Option<&str>,
    ) -> Result<CompletionResponse, ProviderFailure> {
        // Only locally valid values are sent, so a 400 naming a sent field
        // really means the alias does not support it.
        wire::check(request, capabilities).map_err(|reason_code| ProviderFailure {
            kind: ProviderFailureKind::Permanent,
            reason_code,
        })?;
        let body = wire::chat_body(request, capabilities);
        match self.post(&body, request_id).await {
            Ok(reply) => {
                let unfence =
                    request.output_schema.is_some() && body.get("response_format").is_none();
                wire::parse_completion(&reply.body, request, unfence)
            }
            Err(PostError::Failed(failure)) => Err(failure),
            // Kanata answers an over-cap size and an unsupported size field
            // with the same 400; only a sent value above the published route
            // maximum is a size fault, which must never downgrade the alias.
            Err(PostError::Rejected(field, _))
                if is_size_field(field) && over_published_output(request, capabilities) =>
            {
                Err(ProviderFailure {
                    kind: ProviderFailureKind::Permanent,
                    reason_code: "size-limit",
                })
            }
            Err(PostError::Rejected(field, capability)) if body.get(field).is_some() => {
                self.downgrades.revoke(&request.model, capability);
                Err(ProviderFailure {
                    kind: ProviderFailureKind::CapabilityRejected(capability),
                    reason_code: "field-rejected",
                })
            }
            Err(PostError::Rejected(..)) => Err(ProviderFailure {
                kind: ProviderFailureKind::Permanent,
                reason_code: "field-rejected",
            }),
        }
    }

    async fn post(&self, body: &Value, request_id: Option<&str>) -> Result<Reply, PostError> {
        let bytes = serde_json::to_vec(body).map_err(|_| {
            PostError::Failed(ProviderFailure {
                kind: ProviderFailureKind::Permanent,
                reason_code: "request",
            })
        })?;
        let reply = self
            .transport
            .send(
                Method::POST,
                "chat/completions",
                Some(bytes),
                request_id,
                self.limits.completion_body_bytes,
                self.limits.error_body_bytes,
            )
            .await
            .map_err(PostError::Failed)?;
        match reply.status {
            StatusCode::OK => Ok(reply),
            StatusCode::BAD_REQUEST => match rejected_field(&reply.body) {
                Some((field, capability)) => Err(PostError::Rejected(field, capability)),
                None => Err(PostError::Failed(status_failure(&reply))),
            },
            _ => Err(PostError::Failed(status_failure(&reply))),
        }
    }

    fn lock_catalog(&self) -> std::sync::MutexGuard<'_, Catalog> {
        // Replaced wholesale on every write, so a poisoned value is still coherent.
        self.catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl LlmProvider for OpenAiCompatibleProvider {
    /// Direct use resolves capabilities itself; the runner uses `complete_with`.
    fn complete(&self, request: &ChatRequest) -> CompletionFuture<'_> {
        let request = request.clone();
        Box::pin(async move {
            let capabilities = self.model_capabilities(&request.model).await;
            self.chat(&request, &capabilities, None).await
        })
    }

    fn complete_with(
        &self,
        request: &ChatRequest,
        capabilities: &ModelCapabilities,
    ) -> CompletionFuture<'_> {
        let request = request.clone();
        let capabilities = capabilities.clone();
        Box::pin(async move { self.chat(&request, &capabilities, None).await })
    }

    fn complete_tagged(
        &self,
        request: &ChatRequest,
        capabilities: &ModelCapabilities,
        request_id: &str,
    ) -> CompletionFuture<'_> {
        let request = request.clone();
        let capabilities = capabilities.clone();
        let request_id = request_id.to_owned();
        Box::pin(async move { self.chat(&request, &capabilities, Some(&request_id)).await })
    }

    /// Half the remaining runner deadline at most, so a stalled listing still
    /// leaves time for the completion itself.
    fn capabilities<'a>(&'a self, model: &'a str, deadline: Instant) -> CapabilityFuture<'a> {
        let budget = self
            .catalog_timeout
            .min(deadline.saturating_duration_since(Instant::now()) / 2);
        Box::pin(async move { Some(self.resolve(model, budget).await) })
    }
}

impl fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("has_key", &self.transport.has_key())
            .field("declared_count", &self.declared.len())
            .field("downgrades", &self.downgrades)
            .finish_non_exhaustive()
    }
}

fn catalog_of(models: &[ListedModel]) -> Models {
    Arc::new(
        models
            .iter()
            .map(|model| (model.id.clone(), model.capabilities.clone()))
            .collect(),
    )
}

/// `error.param` of a 400 body when it names a known optional field.
fn rejected_field(body: &[u8]) -> Option<(&'static str, Capability)> {
    let value: Value = serde_json::from_slice(body).ok()?;
    field_capability(value.get("error")?.get("param")?.as_str()?)
}

fn is_size_field(field: &str) -> bool {
    matches!(field, "max_tokens" | "max_completion_tokens")
}

fn over_published_output(request: &ChatRequest, capabilities: &ModelCapabilities) -> bool {
    capabilities
        .max_output_tokens
        .is_some_and(|maximum| request.max_output_tokens > maximum)
}

/// Kanata admission refusals, each pinned to the status Kanata sends it with: the
/// request was turned away before any backend work.
const ADMISSION_CODES: [(u16, &str); 4] = [
    (429, "gateway_queue_full"),
    (503, "gateway_busy"),
    (429, "gateway_key_busy"),
    (429, "gateway_key_rate_limited"),
];

/// Distinct from other 401s so operators can tell a rotated-out key.
pub(crate) const KEY_EXPIRED: &str = "key-expired";

/// `error.code` of an OpenAI-style error body.
fn gateway_code(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    Some(value.get("error")?.get("code")?.as_str()?.to_owned())
}

fn status_failure(reply: &Reply) -> ProviderFailure {
    let status = reply.status.as_u16();
    let code = match status {
        401 | 429 | 500..=599 => gateway_code(&reply.body),
        _ => None,
    };
    let (kind, reason_code) = match (status, code.as_deref()) {
        (401, Some("key_expired")) => (ProviderFailureKind::Authentication, KEY_EXPIRED),
        (401 | 403, _) => (ProviderFailureKind::Authentication, "authentication"),
        (_, Some(code)) if ADMISSION_CODES.contains(&(status, code)) => (
            ProviderFailureKind::AdmissionRefused {
                retry_after: reply.retry_after,
            },
            "admission",
        ),
        // Shutting down, not failing: requeue shortly, breaker-neutral.
        (503, Some("server_draining")) => (
            ProviderFailureKind::AdmissionRefused {
                retry_after: reply.retry_after,
            },
            "draining",
        ),
        // With Retry-After this is Kanata's own open breaker; without it the
        // adapter is down or unbound, which counts toward our threshold instead.
        (503, Some("upstream_unavailable")) => match reply.retry_after {
            Some(retry_after) => (
                ProviderFailureKind::BackendUnavailable {
                    retry_after: Some(retry_after),
                },
                "backend-unavailable",
            ),
            None => (ProviderFailureKind::Transient, "backend-unavailable"),
        },
        // Kanata queue timeouts are 503 gateway_busy; a 504 is a backend phase timing out.
        (504, _) => (ProviderFailureKind::UpstreamTimeout, "upstream-timeout"),
        (429, _) => match reply.retry_after {
            Some(retry_after) => (
                ProviderFailureKind::RateLimited { retry_after },
                "rate-limited",
            ),
            None => (ProviderFailureKind::Transient, "rate-limited"),
        },
        (500..=599, _) => (ProviderFailureKind::Transient, "server-error"),
        (300..=399, _) => (ProviderFailureKind::Permanent, "redirect"),
        _ => (ProviderFailureKind::Permanent, "status"),
    };
    ProviderFailure { kind, reason_code }
}
