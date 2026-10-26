//! GETs to `https://open.api.nexon.com/maplestorysea/v1/…` on the crate's
//! hyper + tokio-rustls (ring, webpki roots) stack, as the Discord CDN
//! client does: one connection per call, a whole-exchange timeout, a capped
//! body, redirects never followed. The key travels only in the
//! `x-nxopen-api-key` header and is redacted everywhere else. One call per
//! method; pacing, the budget and retries are the caller's.

use std::{fmt, pin::pin, sync::Arc, time::Duration};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{
    Request,
    body::Bytes,
    client::conn::http1,
    header::{ACCEPT, HOST, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider, pki_types::ServerName};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::wire::{Basic, IdBody, StatBody, combat_power, error_code};
use kanade_domain::chardata::Failure;

pub const HOST_NAME: &str = "open.api.nexon.com";
pub const KEY_HEADER: &str = "x-nxopen-api-key";
const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_BODY: usize = 256 * 1024;

/// The Open API key, from the secrets file `nexon_api_key`.
#[derive(Clone)]
pub struct ApiKey(Arc<str>);

impl ApiKey {
    pub fn new(key: &str) -> Self {
        Self(key.into())
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ApiKey(..)")
    }
}

/// A failed call: the Open API error code when the body carried one, the
/// HTTP status when there was an answer. Never a message or a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallError {
    pub code: Option<String>,
    pub status: Option<u16>,
}

impl CallError {
    fn unanswered() -> Self {
        Self {
            code: None,
            status: None,
        }
    }

    pub fn failure(&self) -> Failure {
        Failure::classify(self.code.as_deref(), self.status)
    }

    /// For logs and refresh state: the code, else `http_<status>`, else
    /// `unavailable`.
    pub fn label(&self) -> String {
        match (&self.code, self.status) {
            (Some(code), _) => code.clone(),
            (None, Some(status)) => format!("http_{status}"),
            (None, None) => "unavailable".into(),
        }
    }
}

pub struct NexonClient {
    target: Target,
    key: ApiKey,
}

enum Target {
    Tls(TlsConnector),
    /// A loopback fake speaking plain HTTP (tests only).
    #[cfg(any(test, feature = "test-support"))]
    Plain(std::net::SocketAddr),
}

impl fmt::Debug for NexonClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NexonClient(..)")
    }
}

impl NexonClient {
    /// `None` when no Rustls crypto provider is installed.
    pub fn new(key: ApiKey) -> Option<Self> {
        let provider: Arc<CryptoProvider> = CryptoProvider::get_default().cloned()?;
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .ok()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Some(Self {
            target: Target::Tls(TlsConnector::from(Arc::new(config))),
            key,
        })
    }

    /// A client of a loopback fake Open API.
    #[cfg(any(test, feature = "test-support"))]
    pub fn plain(address: std::net::SocketAddr, key: ApiKey) -> Self {
        Self {
            target: Target::Plain(address),
            key,
        }
    }

    /// `/id?character_name=`: the ocid, `None` for an empty answer.
    pub async fn id(&self, character_name: &str) -> Result<Option<String>, CallError> {
        let body = self
            .get(&format!(
                "/maplestorysea/v1/id?character_name={}",
                encode(character_name)
            ))
            .await?;
        let parsed: IdBody = serde_json::from_slice(&body).map_err(|_| CallError::unanswered())?;
        Ok(parsed.ocid.filter(|ocid| !ocid.is_empty()))
    }

    /// `/character/basic?ocid=` (no `date`: the latest data).
    pub async fn basic(&self, ocid: &str) -> Result<Basic, CallError> {
        let body = self
            .get(&format!(
                "/maplestorysea/v1/character/basic?ocid={}",
                encode(ocid)
            ))
            .await?;
        serde_json::from_slice(&body).map_err(|_| CallError::unanswered())
    }

    /// `/character/stat?ocid=`: combat power only.
    pub async fn combat_power(&self, ocid: &str) -> Result<Option<u64>, CallError> {
        let body = self
            .get(&format!(
                "/maplestorysea/v1/character/stat?ocid={}",
                encode(ocid)
            ))
            .await?;
        let parsed: StatBody =
            serde_json::from_slice(&body).map_err(|_| CallError::unanswered())?;
        Ok(parsed.final_stat.as_deref().and_then(combat_power))
    }

    async fn get(&self, path: &str) -> Result<Bytes, CallError> {
        let request = Request::get(path)
            .header(HOST, HOST_NAME)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, concat!("kanade/", env!("CARGO_PKG_VERSION")))
            .header(KEY_HEADER, &*self.key.0)
            .body(Empty::<Bytes>::new())
            .map_err(|_| CallError::unanswered())?;
        tokio::time::timeout(TIMEOUT, async {
            match &self.target {
                Target::Tls(tls) => {
                    let tcp = TcpStream::connect((HOST_NAME, 443))
                        .await
                        .map_err(|_| CallError::unanswered())?;
                    let name =
                        ServerName::try_from(HOST_NAME).map_err(|_| CallError::unanswered())?;
                    let stream = tls
                        .connect(name, tcp)
                        .await
                        .map_err(|_| CallError::unanswered())?;
                    exchange(stream, request).await
                }
                #[cfg(any(test, feature = "test-support"))]
                Target::Plain(address) => {
                    let tcp = TcpStream::connect(address)
                        .await
                        .map_err(|_| CallError::unanswered())?;
                    exchange(tcp, request).await
                }
            }
        })
        .await
        .map_err(|_| CallError::unanswered())?
    }
}

async fn exchange<IO>(io: IO, request: Request<Empty<Bytes>>) -> Result<Bytes, CallError>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = http1::Builder::new()
        .handshake(TokioIo::new(io))
        .await
        .map_err(|_| CallError::unanswered())?;
    let mut connection = pin!(connection);
    let mut exchange = pin!(async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| CallError::unanswered())?;
        let status = response.status().as_u16();
        let body = Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|_| CallError {
                code: None,
                status: Some(status),
            })?
            .to_bytes();
        // 3xx is never followed.
        if status == 200 {
            Ok(body)
        } else {
            Err(CallError {
                code: error_code(&body),
                status: Some(status),
            })
        }
    });
    tokio::select! {
        biased;
        reply = &mut exchange => reply,
        _ = &mut connection => exchange.await,
    }
}

/// Percent-encode a query value (every byte outside RFC 3986 unreserved).
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_percent_encoded_and_the_key_is_redacted() {
        assert_eq!(encode("Wisteria"), "Wisteria");
        assert_eq!(encode("a b&c=é"), "a%20b%26c%3D%C3%A9");
        let key = ApiKey::new("live_secret_key");
        assert!(!format!("{key:?}").contains("secret"));
    }
}
