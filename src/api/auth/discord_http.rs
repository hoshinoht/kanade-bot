//! HTTPS [`DiscordApi`] on the crate's hyper + tokio-rustls (ring, webpki
//! roots) stack, following the provider transport: one connection per call,
//! a whole-exchange timeout, capped bodies, redirects never followed.

use std::{pin::pin, sync::Arc, time::Duration};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, StatusCode,
    body::Bytes,
    client::conn::http1,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST, RETRY_AFTER, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider, pki_types::ServerName};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::{
    crypto,
    discord::{
        AccessToken, CodeExchange, DiscordApi, DiscordClient, DiscordError, DiscordFuture,
        DiscordUser, TokenGrant,
    },
    wire,
};

const HOST_NAME: &str = "discord.com";
const TOKEN_PATH: &str = "/api/v10/oauth2/token";
const REVOKE_PATH: &str = "/api/v10/oauth2/token/revoke";
const USER_PATH: &str = "/api/v10/users/@me";
const MAX_BODY: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Used when a 429 carries no usable `retry_after`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(60);

pub struct HttpsDiscord {
    tls: TlsConnector,
}

enum Credential<'a> {
    Bearer(&'a AccessToken),
    /// RFC 6749 §2.3.1: form-encoded id and secret, then base64.
    Client(&'a DiscordClient),
}

fn build(
    method: Method,
    path: &str,
    credential: Credential<'_>,
    form: Option<String>,
) -> Result<Request<Full<Bytes>>, DiscordError> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, HOST_NAME)
        .header(ACCEPT, "application/json")
        .header(USER_AGENT, concat!("kanade/", env!("CARGO_PKG_VERSION")));
    if form.is_some() {
        builder = builder.header(CONTENT_TYPE, "application/x-www-form-urlencoded");
    }
    match credential {
        Credential::Bearer(token) => {
            builder = builder.header(AUTHORIZATION, format!("Bearer {}", token.expose()));
        }
        Credential::Client(client) => {
            let pair = format!(
                "{}:{}",
                wire::encode(&client.client_id),
                wire::encode(client.client_secret.expose())
            );
            builder = builder.header(
                AUTHORIZATION,
                format!("Basic {}", crypto::base64_standard(pair.as_bytes())),
            );
        }
    }
    builder
        .body(Full::new(Bytes::from(form.unwrap_or_default())))
        .map_err(|_| DiscordError::Invalid)
}

pub fn token_request(exchange: &CodeExchange<'_>) -> Result<Request<Full<Bytes>>, DiscordError> {
    build(
        Method::POST,
        TOKEN_PATH,
        Credential::Client(exchange.client),
        Some(wire::form(&[
            ("grant_type", "authorization_code"),
            ("code", exchange.code),
            ("redirect_uri", &exchange.client.redirect_uri),
            ("code_verifier", exchange.code_verifier),
        ])),
    )
}

pub fn revoke_request(
    client: &DiscordClient,
    token: &AccessToken,
) -> Result<Request<Full<Bytes>>, DiscordError> {
    build(
        Method::POST,
        REVOKE_PATH,
        Credential::Client(client),
        Some(wire::form(&[
            ("token", token.expose()),
            ("token_type_hint", "access_token"),
        ])),
    )
}

impl HttpsDiscord {
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
            tls: TlsConnector::from(Arc::new(config)),
        })
    }

    async fn send(&self, request: Request<Full<Bytes>>) -> Result<Bytes, DiscordError> {
        tokio::time::timeout(TIMEOUT, async {
            let tcp = TcpStream::connect((HOST_NAME, 443))
                .await
                .map_err(|_| DiscordError::Unavailable)?;
            let name = ServerName::try_from(HOST_NAME).map_err(|_| DiscordError::Invalid)?;
            let stream = self
                .tls
                .connect(name, tcp)
                .await
                .map_err(|_| DiscordError::Unavailable)?;
            let (mut sender, connection) = http1::Builder::new()
                .max_buf_size(64 * 1024)
                .handshake(TokioIo::new(stream))
                .await
                .map_err(|_| DiscordError::Unavailable)?;
            let mut connection = pin!(connection);
            let mut exchange = pin!(async move {
                let response = sender
                    .send_request(request)
                    .await
                    .map_err(|_| DiscordError::Unavailable)?;
                let status = response.status();
                let header_wait = retry_after_header(response.headers());
                let body = Limited::new(response.into_body(), MAX_BODY)
                    .collect()
                    .await
                    .map_err(|_| DiscordError::Invalid)?
                    .to_bytes();
                classify(status, header_wait, &body)?;
                Ok(body)
            });
            tokio::select! {
                biased;
                reply = &mut exchange => reply,
                _ = &mut connection => exchange.await,
            }
        })
        .await
        .map_err(|_| DiscordError::Unavailable)?
    }
}

fn retry_after_header(headers: &HeaderMap) -> Option<Duration> {
    let seconds: f64 = headers
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds.min(3600.0)))
}

#[derive(Deserialize)]
struct RateLimitBody {
    retry_after: f64,
}

pub fn classify(
    status: StatusCode,
    header_wait: Option<Duration>,
    body: &[u8],
) -> Result<(), DiscordError> {
    if status.is_success() {
        Ok(())
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        let body_wait = serde_json::from_slice::<RateLimitBody>(body)
            .ok()
            .map(|body| body.retry_after)
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .map(|seconds| Duration::from_secs_f64(seconds.min(3600.0)));
        Err(DiscordError::RateLimited(
            body_wait.or(header_wait).unwrap_or(DEFAULT_RETRY_AFTER),
        ))
    } else if status.is_server_error() {
        Err(DiscordError::Unavailable)
    } else {
        // 3xx is never followed; 4xx means the code or credentials were refused.
        Err(DiscordError::Rejected)
    }
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    token_type: String,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct UserReply {
    id: String,
    username: String,
    global_name: Option<String>,
    #[serde(default)]
    bot: bool,
}

impl DiscordApi for HttpsDiscord {
    fn exchange_code<'a>(
        &'a self,
        exchange: CodeExchange<'a>,
    ) -> DiscordFuture<'a, Result<TokenGrant, DiscordError>> {
        Box::pin(async move {
            let body = self.send(token_request(&exchange)?).await?;
            let reply: TokenReply =
                serde_json::from_slice(&body).map_err(|_| DiscordError::Invalid)?;
            if !reply.token_type.eq_ignore_ascii_case("bearer") {
                return Err(DiscordError::Invalid);
            }
            Ok(TokenGrant {
                token: AccessToken::new(reply.access_token),
                scope: reply.scope,
            })
        })
    }

    fn current_user<'a>(
        &'a self,
        token: &'a AccessToken,
    ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>> {
        Box::pin(async move {
            let request = build(Method::GET, USER_PATH, Credential::Bearer(token), None)?;
            let body = self.send(request).await?;
            let reply: UserReply =
                serde_json::from_slice(&body).map_err(|_| DiscordError::Invalid)?;
            if reply.id.is_empty() || !reply.id.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(DiscordError::Invalid);
            }
            Ok(DiscordUser {
                id: reply.id,
                username: reply.username,
                global_name: reply.global_name,
                bot: reply.bot,
            })
        })
    }

    fn revoke<'a>(
        &'a self,
        client: &'a DiscordClient,
        token: AccessToken,
    ) -> DiscordFuture<'a, Result<(), DiscordError>> {
        Box::pin(async move {
            self.send(revoke_request(client, &token)?).await?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;
    use crate::api::auth::discord::Secret;

    fn client() -> DiscordClient {
        DiscordClient {
            client_id: "42".into(),
            client_secret: Secret::new("s3cret:+/"),
            redirect_uri: "https://kanade.test/api/admin/auth/discord/callback".into(),
        }
    }

    async fn body(request: Request<Full<Bytes>>) -> String {
        String::from_utf8(
            request
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn client_credentials_travel_only_in_http_basic() {
        let client = client();
        let request = token_request(&CodeExchange {
            client: &client,
            code: "abc",
            code_verifier: "verifier",
        })
        .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.uri(), TOKEN_PATH);
        assert_eq!(
            request.headers()[CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        // base64("42:s3cret%3A%2B%2F")
        assert_eq!(
            request.headers()[AUTHORIZATION],
            format!("Basic {}", crypto::base64_standard(b"42:s3cret%3A%2B%2F"))
        );
        assert_eq!(
            body(request).await,
            "grant_type=authorization_code&code=abc&redirect_uri=https%3A%2F%2Fkanade.test%2Fapi%2Fadmin%2Fauth%2Fdiscord%2Fcallback\
             &code_verifier=verifier"
        );

        let revoke = revoke_request(&client, &AccessToken::new("tok")).unwrap();
        assert_eq!(revoke.uri(), REVOKE_PATH);
        assert!(
            revoke.headers()[AUTHORIZATION]
                .to_str()
                .unwrap()
                .starts_with("Basic ")
        );
        let form = body(revoke).await;
        assert_eq!(form, "token=tok&token_type_hint=access_token");
        assert!(!form.contains("s3cret"));
    }

    #[test]
    fn statuses_and_rate_limits_classify() {
        assert_eq!(classify(StatusCode::OK, None, b""), Ok(()));
        assert_eq!(
            classify(StatusCode::BAD_REQUEST, None, b""),
            Err(DiscordError::Rejected)
        );
        assert_eq!(
            classify(StatusCode::FOUND, None, b""),
            Err(DiscordError::Rejected)
        );
        assert_eq!(
            classify(StatusCode::BAD_GATEWAY, None, b""),
            Err(DiscordError::Unavailable)
        );
        assert_eq!(
            classify(
                StatusCode::TOO_MANY_REQUESTS,
                Some(Duration::from_secs(9)),
                br#"{"retry_after": 2.5}"#
            ),
            Err(DiscordError::RateLimited(Duration::from_millis(2500)))
        );
        assert_eq!(
            classify(
                StatusCode::TOO_MANY_REQUESTS,
                Some(Duration::from_secs(9)),
                b"x"
            ),
            Err(DiscordError::RateLimited(Duration::from_secs(9)))
        );
        assert_eq!(
            classify(
                StatusCode::TOO_MANY_REQUESTS,
                None,
                br#"{"retry_after": -1}"#
            ),
            Err(DiscordError::RateLimited(DEFAULT_RETRY_AFTER))
        );
    }

    #[test]
    fn user_reply_carries_the_bot_flag() {
        let reply: UserReply =
            serde_json::from_str(r#"{"id":"1","username":"b","global_name":null,"bot":true}"#)
                .unwrap();
        assert!(reply.bot);
        let reply: UserReply =
            serde_json::from_str(r#"{"id":"1","username":"u","global_name":null}"#).unwrap();
        assert!(!reply.bot);
        let reply: TokenReply =
            serde_json::from_str(r#"{"access_token":"a","token_type":"Bearer"}"#).unwrap();
        assert_eq!(reply.scope, "", "a missing scope is not identify");
    }
}
