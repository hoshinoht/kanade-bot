//! The running bot's admin API over its own listener, for one command: a
//! break-glass sign-in, the session cookie and CSRF token kept in memory
//! only, then a sign-out whatever happened in between. Error text never
//! carries the token, the cookie or the CSRF token.

use std::{
    future::poll_fn,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::pin,
    task::Poll,
    time::Duration,
};

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, StatusCode,
    body::Bytes,
    client::conn::http1,
    header::{ACCEPT, CONTENT_TYPE, COOKIE, HOST, ORIGIN, SET_COOKIE, USER_AGENT},
};
use hyper_util::rt::TokioIo;
use serde_json::Value;
use tokio::net::TcpStream;

use crate::{
    api::auth::{csrf::CSRF_HEADER, wire::SESSION_COOKIE},
    runtime::{
        config::{ADMIN_TOKEN_FILE_KEY, AdminClientConfig},
        error::Error,
        secrets::Redacted,
    },
};

/// Per request, connect to last byte.
const TIMEOUT: Duration = Duration::from_secs(10);
/// The Config view is the largest answer read here.
const MAX_BODY: usize = 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
/// API error messages are short, fixed sentences; anything longer is cut.
const MAX_MESSAGE_CHARS: usize = 200;
const IDEMPOTENCY_KEY: &str = "idempotency-key";

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl Reply {
    fn json(&self, what: &str) -> Result<Value, Error> {
        serde_json::from_slice(&self.body).map_err(|_| {
            Error::Unavailable(format!("{what}: the admin API answered with invalid JSON"))
        })
    }
}

/// A signed-in session of this command.
pub struct Session {
    config: AdminClientConfig,
    cookie: String,
    csrf: String,
}

/// Sign in with the token from `KANADE_ADMIN_TOKEN_FILE`, run `work`, then
/// sign out, also when `work` failed or panicked (a panic is reported as a
/// failure). Nothing is attempted when the sign-in fails. A failed sign-out
/// after successful work is itself an error: the session then ends only at
/// the idle limit.
///
/// # Errors
/// The token file, the sign-in, `work` or the sign-out failed, or `work`
/// panicked; the message never contains the token, the cookie or the CSRF
/// token.
pub async fn signed_in(
    config: &AdminClientConfig,
    work: impl AsyncFnOnce(&Session) -> Result<(), Error>,
) -> Result<(), Error> {
    let token = Redacted::read(&config.token_file, ADMIN_TOKEN_FILE_KEY)?;
    let session = sign_in(config, &token)
        .await
        .map_err(|error| scrub(error, &[token.expose()]))?;
    // A panic in `work` is caught where it is polled, so the sign-out below
    // is still sent; the panic hook has already printed the panic itself.
    let mut working = pin!(work(&session));
    let worked = poll_fn(|cx| {
        catch_unwind(AssertUnwindSafe(|| working.as_mut().poll(cx))).unwrap_or_else(|_| {
            Poll::Ready(Err(Error::Unavailable(
                "the command stopped unexpectedly (a bug; see the panic above)".into(),
            )))
        })
    })
    .await;
    let ended = session
        .call(Method::POST, "/api/admin/auth/logout", None, "sign-out")
        .await;
    let secrets = [
        token.expose(),
        session.cookie.as_str(),
        session.csrf.as_str(),
    ];
    match (worked, ended) {
        (Err(error), _) => Err(scrub(error, &secrets)),
        (Ok(()), Err(error)) => Err(scrub(
            with_suffix(
                error,
                "; this command's session ends by itself at the admin idle limit",
            ),
            &secrets,
        )),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

async fn sign_in(config: &AdminClientConfig, token: &Redacted) -> Result<Session, Error> {
    let body = serde_json::json!({ "token": token.expose() }).to_string();
    let reply = send(
        config,
        Method::POST,
        "/api/admin/auth/token",
        None,
        Some(body),
        None,
    )
    .await
    .map_err(|error| prefixed("sign-in", error))?;
    if reply.status != StatusCode::OK {
        return Err(refused("sign-in", &reply));
    }
    let csrf = reply
        .headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let cookie = reply
        .headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let pair = value.split(';').next()?.trim();
            pair.strip_prefix(SESSION_COOKIE)?
                .strip_prefix('=')
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        });
    match (cookie, csrf) {
        (Some(cookie), Some(csrf)) => Ok(Session {
            config: config.clone(),
            cookie,
            csrf,
        }),
        _ => Err(Error::Unavailable(
            "sign-in: the admin API answered without a session".into(),
        )),
    }
}

impl Session {
    /// `method path` with this session, expecting a 2xx; `what` names the
    /// step in errors. Returns the JSON body (`Null` when empty).
    ///
    /// # Errors
    /// Unreachable or slow API, a refusal or a non-JSON body.
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        what: &str,
    ) -> Result<Value, Error> {
        // The Config PATCH honours `Idempotency-Key`: a fresh one per run,
        // so a replayed request is never applied twice.
        let key = (method == Method::PATCH).then(|| format!("kanade-cli-{}", uuid::Uuid::new_v4()));
        let reply = send(
            &self.config,
            method,
            path,
            Some(self),
            body.map(|body| body.to_string()),
            key.as_deref(),
        )
        .await
        .map_err(|error| prefixed(what, error))?;
        if !reply.status.is_success() {
            return Err(refused(what, &reply));
        }
        if reply.body.is_empty() {
            return Ok(Value::Null);
        }
        reply.json(what)
    }
}

async fn send(
    config: &AdminClientConfig,
    method: Method,
    path: &str,
    session: Option<&Session>,
    body: Option<String>,
    idempotency_key: Option<&str>,
) -> Result<Reply, Error> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, &config.authority)
        // The listener's own origin: what the CSRF check requires of a
        // browser on an unsafe request.
        .header(ORIGIN, &config.origin)
        .header(
            USER_AGENT,
            concat!("kanade-cli/", env!("CARGO_PKG_VERSION")),
        )
        .header(ACCEPT, "application/json");
    if let Some(session) = session {
        request = request
            .header(COOKIE, format!("{SESSION_COOKIE}={}", session.cookie))
            .header(CSRF_HEADER, &session.csrf);
    }
    if body.is_some() {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    if let Some(key) = idempotency_key {
        request = request.header(IDEMPOTENCY_KEY, key);
    }
    let request = request
        .body(Full::new(Bytes::from(body.unwrap_or_default())))
        .map_err(|_| Error::Unavailable("the request could not be built".into()))?;
    tokio::time::timeout(TIMEOUT, exchange(config, request))
        .await
        .map_err(|_| {
            Error::Unavailable(format!(
                "the admin API at {} did not answer in time",
                config.target
            ))
        })?
}

async fn exchange(
    config: &AdminClientConfig,
    request: Request<Full<Bytes>>,
) -> Result<Reply, Error> {
    let unreachable = || {
        Error::Unavailable(format!(
            "the admin API at {} is unreachable (is the bot running?)",
            config.target
        ))
    };
    let stream = TcpStream::connect(config.target)
        .await
        .map_err(|_| unreachable())?;
    let (mut sender, connection) = http1::Builder::new()
        .max_buf_size(MAX_HEADER_BYTES)
        .handshake(TokioIo::new(stream))
        .await
        .map_err(|_| unreachable())?;
    let mut connection = pin!(connection);
    let mut reply = pin!(async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| unreachable())?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = Limited::new(response.into_body(), MAX_BODY)
            .collect()
            .await
            .map_err(|_| {
                Error::Unavailable("the admin API answer was unreadable or too large".into())
            })?
            .to_bytes();
        Ok(Reply {
            status,
            headers,
            body,
        })
    });
    tokio::select! {
        biased;
        reply = &mut reply => reply,
        _ = &mut connection => reply.await,
    }
}

/// A refusal as an error: sign-in and CSRF/Host refusals are setup problems
/// (78), throttling and server faults are retryable (69), other refusals
/// are the request's own (64). The API's message is fixed, value-free text.
fn refused(what: &str, reply: &Reply) -> Error {
    let status = reply.status;
    let (code, message) = reply
        .json(what)
        .ok()
        .map(|body| {
            let text = |key: &str| body[key].as_str().map(clean).unwrap_or_default();
            (text("error"), text("message"))
        })
        .unwrap_or_default();
    let mut detail = format!("{what}: the admin API answered {}", status.as_u16());
    match (code.is_empty(), message.is_empty()) {
        (false, false) => detail.push_str(&format!(" {code}: {message}")),
        (false, true) => detail.push_str(&format!(" {code}")),
        (true, false) => detail.push_str(&format!(": {message}")),
        (true, true) => {}
    }
    match status {
        StatusCode::UNAUTHORIZED if what == "sign-in" => Error::Configuration(format!(
            "{detail}; check that {ADMIN_TOKEN_FILE_KEY} holds the bot's current token"
        )),
        StatusCode::MISDIRECTED_REQUEST => Error::Configuration(format!(
            "{detail}; set KANADE_ADMIN_HOST to the bot's admin host"
        )),
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Error::Configuration(detail),
        StatusCode::TOO_MANY_REQUESTS => Error::Unavailable(detail),
        status if status.is_client_error() => Error::Usage(detail),
        _ => Error::Unavailable(detail),
    }
}

/// One line of at most [`MAX_MESSAGE_CHARS`] printable characters.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

fn rebuild(error: Error, edit: impl FnOnce(String) -> String) -> Error {
    match error {
        Error::Configuration(message) => Error::Configuration(edit(message)),
        Error::Unavailable(message) => Error::Unavailable(edit(message)),
        Error::Usage(message) => Error::Usage(edit(message)),
        Error::Startup(message) => Error::Startup(edit(message)),
    }
}

fn prefixed(what: &str, error: Error) -> Error {
    rebuild(error, |message| format!("{what}: {message}"))
}

fn with_suffix(error: Error, suffix: &str) -> Error {
    rebuild(error, |message| message + suffix)
}

/// `error` with every non-empty secret replaced, whatever produced the text.
fn scrub(error: Error, secrets: &[&str]) -> Error {
    rebuild(error, |message| {
        secrets
            .iter()
            .filter(|secret| !secret.is_empty())
            .fold(message, |message, secret| {
                message.replace(secret, "[redacted]")
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status: StatusCode::from_u16(status).unwrap(),
            headers: HeaderMap::new(),
            body: Bytes::from(body.to_owned()),
        }
    }

    #[test]
    fn refusals_map_to_exit_codes_and_keep_only_clean_api_text() {
        let error = refused(
            "sign-in",
            &reply(
                401,
                r#"{"error":"unauthenticated","message":"Sign in first."}"#,
            ),
        );
        assert_eq!(error.exit_code(), 78);
        assert!(
            error.to_string().starts_with(
                "sign-in: the admin API answered 401 unauthenticated: Sign in first.;"
            ),
            "{error}"
        );
        assert_eq!(refused("revoke", &reply(404, "{}")).exit_code(), 64);
        assert_eq!(refused("revoke", &reply(409, "")).exit_code(), 64);
        assert_eq!(refused("portal open", &reply(403, "")).exit_code(), 78);
        assert_eq!(refused("portal open", &reply(421, "")).exit_code(), 78);
        assert_eq!(refused("portal open", &reply(429, "")).exit_code(), 69);
        assert_eq!(refused("portal open", &reply(503, "")).exit_code(), 69);
        let long = format!(r#"{{"error":"x","message":"a\nb{}"}}"#, "c".repeat(500));
        let text = refused("portal open", &reply(500, &long)).to_string();
        assert!(!text.contains('\n'));
        assert!(text.len() < 300, "{text}");
    }

    #[test]
    fn scrub_removes_every_secret_from_any_kind() {
        let error = scrub(
            Error::Usage("token hunter2 cookie c00kie and hunter2".into()),
            &["hunter2", "c00kie", ""],
        );
        assert_eq!(error.exit_code(), 64);
        assert_eq!(
            error.to_string(),
            "token [redacted] cookie [redacted] and [redacted]"
        );
    }
}
