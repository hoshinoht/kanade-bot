use std::{pin::pin, sync::Arc, time::Duration};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, StatusCode,
    body::{Body, Bytes},
    client::conn::http1,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST, RETRY_AFTER, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpStream, lookup_host},
};
use tokio_rustls::TlsConnector;

use super::super::{ProviderFailure, ProviderFailureKind};
use super::{
    config::{BearerKey, HttpConfigError, TrustRoots},
    endpoint::Endpoint,
};

const MAX_HEADERS: usize = 64;
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Longest `Retry-After` honoured; the caller's deadline bounds it further.
const MAX_RETRY_AFTER: u64 = 3_600;

pub(crate) struct Reply {
    pub status: StatusCode,
    /// Empty for redirects and for error bodies over their cap.
    pub body: Bytes,
    /// `Retry-After` in delta-seconds; HTTP-date values are ignored.
    pub retry_after: Option<Duration>,
}

/// One connection per exchange; redirects are returned, never followed.
pub(crate) struct Transport {
    endpoint: Endpoint,
    tls: Option<TlsConnector>,
    bearer: Option<BearerKey>,
    timeout: Duration,
}

impl Transport {
    pub fn new(
        endpoint: Endpoint,
        roots: &TrustRoots,
        bearer: Option<BearerKey>,
        timeout: Duration,
    ) -> Result<Self, HttpConfigError> {
        let tls = match endpoint.server_name {
            Some(_) => Some(connector(roots)?),
            None => None,
        };
        Ok(Self {
            endpoint,
            tls,
            bearer,
            timeout,
        })
    }

    pub fn has_key(&self) -> bool {
        self.bearer.is_some()
    }

    pub async fn send(
        &self,
        method: Method,
        suffix: &str,
        body: Option<Vec<u8>>,
        success_limit: usize,
        error_limit: usize,
    ) -> Result<Reply, ProviderFailure> {
        let request = self.request(method, suffix, body)?;
        tokio::time::timeout(self.timeout, async {
            let tcp = self.connect().await?;
            match (&self.tls, &self.endpoint.server_name) {
                (Some(tls), Some(name)) => {
                    let stream = tls
                        .connect(name.clone(), tcp)
                        .await
                        .map_err(|error| handshake_failure(&error))?;
                    exchange(stream, request, success_limit, error_limit).await
                }
                _ => exchange(tcp, request, success_limit, error_limit).await,
            }
        })
        .await
        // One timeout covers the whole exchange, so the request may have reached the backend.
        .map_err(|_| failure(ProviderFailureKind::UpstreamTimeout, "timeout"))?
    }

    fn request(
        &self,
        method: Method,
        suffix: &str,
        body: Option<Vec<u8>>,
    ) -> Result<Request<Full<Bytes>>, ProviderFailure> {
        let mut builder = Request::builder()
            .method(method)
            .uri(self.endpoint.path(suffix))
            .header(HOST, self.endpoint.authority.as_str())
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, concat!("kanade/", env!("CARGO_PKG_VERSION")));
        if body.is_some() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }
        if let Some(key) = &self.bearer {
            let header = key
                .header()
                .ok_or_else(|| failure(ProviderFailureKind::Authentication, "key"))?;
            builder = builder.header(AUTHORIZATION, header);
        }
        builder
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .map_err(|_| failure(ProviderFailureKind::Permanent, "request"))
    }

    async fn connect(&self) -> Result<TcpStream, ProviderFailure> {
        let unreachable = || failure(ProviderFailureKind::Transient, "connect");
        let addresses = lookup_host((self.endpoint.host.as_str(), self.endpoint.port))
            .await
            .map_err(|_| unreachable())?;
        for address in addresses {
            if self.endpoint.loopback_only && !address.ip().is_loopback() {
                continue;
            }
            if let Ok(stream) = TcpStream::connect(address).await {
                let _ = stream.set_nodelay(true);
                return Ok(stream);
            }
        }
        Err(unreachable())
    }
}

fn connector(roots: &TrustRoots) -> Result<TlsConnector, HttpConfigError> {
    let provider: Arc<CryptoProvider> = CryptoProvider::get_default()
        .cloned()
        .ok_or(HttpConfigError::CryptoProviderMissing)?;
    let store = match roots {
        TrustRoots::WebPki => RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        },
        TrustRoots::Custom(certificates) => {
            let mut store = RootCertStore::empty();
            for certificate in certificates {
                store
                    .add(certificate.clone())
                    .map_err(|_| HttpConfigError::InvalidTrustRoots)?;
            }
            if store.is_empty() {
                return Err(HttpConfigError::InvalidTrustRoots);
            }
            store
        }
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| HttpConfigError::CryptoProviderMissing)?
        .with_root_certificates(store)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsConnector::from(Arc::new(config)))
}

async fn exchange<S>(
    stream: S,
    request: Request<Full<Bytes>>,
    success_limit: usize,
    error_limit: usize,
) -> Result<Reply, ProviderFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let protocol = || failure(ProviderFailureKind::Transient, "protocol");
    // Once the request is handed over it may have reached the backend, so a lost
    // reply is treated like a timeout: charged, not retried for chat.
    let interrupted = || failure(ProviderFailureKind::UpstreamTimeout, "interrupted");
    let mut builder = http1::Builder::new();
    builder
        .max_headers(MAX_HEADERS)
        .max_buf_size(MAX_HEADER_BYTES);
    let (mut sender, connection) = builder
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| protocol())?;
    // Driven inline rather than spawned so a timeout or cancellation leaves no task.
    let mut connection = pin!(connection);
    let mut exchange = pin!(async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| interrupted())?;
        let status = response.status();
        let retry_after = retry_after(response.headers());
        if status.is_redirection() {
            return Ok(Reply {
                status,
                body: Bytes::new(),
                retry_after,
            });
        }
        let limit = if status == StatusCode::OK {
            success_limit
        } else {
            error_limit
        };
        let declared_over = response
            .body()
            .size_hint()
            .upper()
            .is_some_and(|size| size > limit as u64);
        let body = if declared_over {
            None
        } else {
            match Limited::new(response.into_body(), limit).collect().await {
                Ok(collected) => Some(collected.to_bytes()),
                Err(error)
                    if error
                        .downcast_ref::<http_body_util::LengthLimitError>()
                        .is_some() =>
                {
                    None
                }
                Err(_) => return Err(interrupted()),
            }
        };
        match body {
            Some(body) => Ok(Reply {
                status,
                body,
                retry_after,
            }),
            None if status == StatusCode::OK => {
                Err(failure(ProviderFailureKind::InvalidOutput, "body-size"))
            }
            None => Ok(Reply {
                status,
                body: Bytes::new(),
                retry_after,
            }),
        }
    });
    tokio::select! {
        biased;
        reply = &mut exchange => reply,
        _ = &mut connection => exchange.await,
    }
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seconds = value.parse::<u64>().unwrap_or(u64::MAX);
    Some(Duration::from_secs(seconds.min(MAX_RETRY_AFTER)))
}

/// TLS protocol and certificate errors are permanent; a reset or EOF mid-handshake
/// is a transport hiccup.
fn handshake_failure(error: &std::io::Error) -> ProviderFailure {
    let tls = error
        .get_ref()
        .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some());
    if tls {
        failure(ProviderFailureKind::Permanent, "tls")
    } else {
        failure(ProviderFailureKind::Transient, "tls-io")
    }
}

fn failure(kind: ProviderFailureKind, reason_code: &'static str) -> ProviderFailure {
    ProviderFailure { kind, reason_code }
}
