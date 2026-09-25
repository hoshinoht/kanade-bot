//! HTTPS [`DiscordApi`] on the crate's hyper + tokio-rustls (ring, webpki
//! roots) stack, following the provider transport: one connection per call,
//! a whole-exchange timeout, capped bodies, redirects never followed.

use std::{pin::pin, sync::Arc, time::Duration};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, StatusCode,
    body::Bytes,
    client::conn::http1,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider, pki_types::ServerName};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::{
    discord::{
        AccessToken, CodeExchange, DiscordApi, DiscordClient, DiscordError, DiscordFuture,
        DiscordUser,
    },
    wire,
};

const HOST_NAME: &str = "discord.com";
const TOKEN_PATH: &str = "/api/v10/oauth2/token";
const REVOKE_PATH: &str = "/api/v10/oauth2/token/revoke";
const USER_PATH: &str = "/api/v10/users/@me";
const MAX_BODY: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

pub struct HttpsDiscord {
    tls: TlsConnector,
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

    async fn call(
        &self,
        method: Method,
        path: &str,
        bearer: Option<&str>,
        form: Option<String>,
    ) -> Result<(StatusCode, Bytes), DiscordError> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, HOST_NAME)
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, concat!("kanade/", env!("CARGO_PKG_VERSION")));
        if form.is_some() {
            builder = builder.header(CONTENT_TYPE, "application/x-www-form-urlencoded");
        }
        if let Some(token) = bearer {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let request = builder
            .body(Full::new(Bytes::from(form.unwrap_or_default())))
            .map_err(|_| DiscordError::Invalid)?;
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
                let body = Limited::new(response.into_body(), MAX_BODY)
                    .collect()
                    .await
                    .map_err(|_| DiscordError::Invalid)?
                    .to_bytes();
                Ok((status, body))
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

fn classify(status: StatusCode) -> Result<(), DiscordError> {
    if status.is_success() {
        Ok(())
    } else if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
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
}

#[derive(Deserialize)]
struct UserReply {
    id: String,
    username: String,
    global_name: Option<String>,
}

pub fn token_form(exchange: &CodeExchange<'_>) -> String {
    wire::form(&[
        ("grant_type", "authorization_code"),
        ("code", exchange.code),
        ("redirect_uri", &exchange.client.redirect_uri),
        ("code_verifier", exchange.code_verifier),
        ("client_id", &exchange.client.client_id),
        ("client_secret", exchange.client.client_secret.expose()),
    ])
}

impl DiscordApi for HttpsDiscord {
    fn exchange_code<'a>(
        &'a self,
        exchange: CodeExchange<'a>,
    ) -> DiscordFuture<'a, Result<AccessToken, DiscordError>> {
        Box::pin(async move {
            let (status, body) = self
                .call(Method::POST, TOKEN_PATH, None, Some(token_form(&exchange)))
                .await?;
            classify(status)?;
            let reply: TokenReply =
                serde_json::from_slice(&body).map_err(|_| DiscordError::Invalid)?;
            if !reply.token_type.eq_ignore_ascii_case("bearer") {
                return Err(DiscordError::Invalid);
            }
            Ok(AccessToken::new(reply.access_token))
        })
    }

    fn current_user<'a>(
        &'a self,
        token: &'a AccessToken,
    ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>> {
        Box::pin(async move {
            let (status, body) = self
                .call(Method::GET, USER_PATH, Some(token.expose()), None)
                .await?;
            classify(status)?;
            let reply: UserReply =
                serde_json::from_slice(&body).map_err(|_| DiscordError::Invalid)?;
            if reply.id.is_empty() || !reply.id.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(DiscordError::Invalid);
            }
            Ok(DiscordUser {
                id: reply.id,
                username: reply.username,
                global_name: reply.global_name,
            })
        })
    }

    fn revoke<'a>(
        &'a self,
        client: &'a DiscordClient,
        token: AccessToken,
    ) -> DiscordFuture<'a, ()> {
        Box::pin(async move {
            let form = wire::form(&[
                ("token", token.expose()),
                ("token_type_hint", "access_token"),
                ("client_id", &client.client_id),
                ("client_secret", client.client_secret.expose()),
            ]);
            let _ = self.call(Method::POST, REVOKE_PATH, None, Some(form)).await;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::discord::Secret;

    #[test]
    fn token_request_is_a_form_with_verifier_and_exact_redirect() {
        let client = DiscordClient {
            client_id: "42".into(),
            client_secret: Secret::new("s3cret"),
            redirect_uri: "https://kanade.test/api/admin/auth/discord/callback".into(),
        };
        let form = token_form(&CodeExchange {
            client: &client,
            code: "abc",
            code_verifier: "verifier",
        });
        assert_eq!(
            form,
            "grant_type=authorization_code&code=abc&redirect_uri=https%3A%2F%2Fkanade.test%2Fapi%2Fadmin%2Fauth%2Fdiscord%2Fcallback\
             &code_verifier=verifier&client_id=42&client_secret=s3cret"
        );
        assert_eq!(
            classify(StatusCode::BAD_REQUEST),
            Err(DiscordError::Rejected)
        );
        assert_eq!(classify(StatusCode::FOUND), Err(DiscordError::Rejected));
        assert_eq!(
            classify(StatusCode::BAD_GATEWAY),
            Err(DiscordError::Unavailable)
        );
    }
}
