//! Image GETs from Discord's CDN on the crate's hyper + tokio-rustls (ring,
//! webpki roots) stack, like `api::auth::discord_http`: one connection per
//! image, a whole-exchange timeout, a capped body, redirects never followed.

use std::{pin::pin, sync::Arc, time::Duration};

use http_body_util::{BodyExt, Empty, Limited};
use hyper::{
    Request, StatusCode,
    body::Bytes,
    client::conn::http1,
    header::{ACCEPT, CONTENT_TYPE, HOST, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider, pki_types::ServerName};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::{Image, MAX_IMAGE};

const HOST_NAME: &str = "cdn.discordapp.com";
const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Cdn {
    target: Target,
}

enum Target {
    Tls(TlsConnector),
    /// A loopback stub speaking plain HTTP.
    #[cfg(test)]
    Plain(std::net::SocketAddr),
}

impl Cdn {
    /// `None` when no Rustls crypto provider is installed.
    pub fn new() -> Option<Self> {
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
        })
    }

    #[cfg(test)]
    pub fn plain(address: std::net::SocketAddr) -> Self {
        Self {
            target: Target::Plain(address),
        }
    }

    /// `path` is a CDN path with its query; errors are short log reasons.
    pub async fn get(&self, path: &str) -> Result<Image, &'static str> {
        let request = Request::get(path)
            .header(HOST, HOST_NAME)
            .header(ACCEPT, "image/png, image/webp, image/gif, image/jpeg")
            .header(USER_AGENT, concat!("kanade/", env!("CARGO_PKG_VERSION")))
            .body(Empty::<Bytes>::new())
            .map_err(|_| "invalid_request")?;
        tokio::time::timeout(TIMEOUT, async {
            match &self.target {
                Target::Tls(tls) => {
                    let tcp = TcpStream::connect((HOST_NAME, 443))
                        .await
                        .map_err(|_| "unavailable")?;
                    let name = ServerName::try_from(HOST_NAME).map_err(|_| "invalid_request")?;
                    let stream = tls.connect(name, tcp).await.map_err(|_| "unavailable")?;
                    exchange(stream, request).await
                }
                #[cfg(test)]
                Target::Plain(address) => {
                    let tcp = TcpStream::connect(address)
                        .await
                        .map_err(|_| "unavailable")?;
                    exchange(tcp, request).await
                }
            }
        })
        .await
        .map_err(|_| "timeout")?
    }
}

async fn exchange<IO>(io: IO, request: Request<Empty<Bytes>>) -> Result<Image, &'static str>
where
    IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = http1::Builder::new()
        .handshake(TokioIo::new(io))
        .await
        .map_err(|_| "unavailable")?;
    let mut connection = pin!(connection);
    let mut exchange = pin!(async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| "unavailable")?;
        // 3xx is never followed.
        if response.status() != StatusCode::OK {
            return Err("status");
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = Limited::new(response.into_body(), MAX_IMAGE)
            .collect()
            .await
            .map_err(|_| "oversize_or_truncated")?
            .to_bytes()
            .to_vec();
        Ok(Image {
            content_type,
            bytes,
        })
    });
    tokio::select! {
        biased;
        reply = &mut exchange => reply,
        _ = &mut connection => exchange.await,
    }
}
