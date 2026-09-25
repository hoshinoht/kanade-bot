//! Discord OAuth2 authorization-code flow with PKCE S256 and scope `identify`.
//! Discord endpoints: https://docs.discord.com/developers/topics/oauth2 (token
//! and revocation take form bodies; client credentials may be in the body);
//! PKCE parameters: https://docs.discord.food/topics/oauth2#pkce (S256 only).
//! The access token is used for one `/users/@me` call, then revoked.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, TimeDelta, Utc};

use super::{crypto, wire};

pub const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";
pub const CALLBACK_PATH: &str = "/api/admin/auth/discord/callback";
/// How long a started login may take to come back.
pub const LOGIN_TTL: TimeDelta = TimeDelta::minutes(10);
/// Bounds memory held for unfinished logins.
const MAX_PENDING: usize = 64;

pub type DiscordFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A value that must never be logged or echoed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[derive(Clone, Debug)]
pub struct DiscordClient {
    pub client_id: String,
    pub client_secret: Secret,
    /// Exact registered redirect URI.
    pub redirect_uri: String,
}

/// A user access token; not `Clone`, so it is consumed by revocation.
pub struct AccessToken(Secret);

impl AccessToken {
    pub fn new(value: impl Into<String>) -> Self {
        Self(Secret::new(value))
    }

    pub fn expose(&self) -> &str {
        self.0.expose()
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(..)")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
}

impl DiscordUser {
    /// Display name without control characters, bounded for storage.
    pub fn display(&self) -> String {
        let name = self
            .global_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.username);
        name.chars().filter(|c| !c.is_control()).take(100).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscordError {
    /// Discord refused (bad, used or expired code; bad client credentials).
    Rejected,
    /// Network, TLS, timeout or a 5xx.
    Unavailable,
    /// A reply that does not parse.
    Invalid,
}

pub struct CodeExchange<'a> {
    pub client: &'a DiscordClient,
    pub code: &'a str,
    pub code_verifier: &'a str,
}

pub trait DiscordApi: Send + Sync {
    fn exchange_code<'a>(
        &'a self,
        exchange: CodeExchange<'a>,
    ) -> DiscordFuture<'a, Result<AccessToken, DiscordError>>;

    fn current_user<'a>(
        &'a self,
        token: &'a AccessToken,
    ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>>;

    /// Best effort; failures are ignored because the token expires anyway.
    fn revoke<'a>(&'a self, client: &'a DiscordClient, token: AccessToken)
    -> DiscordFuture<'a, ()>;
}

struct Pending {
    state_hash: String,
    verifier: String,
    next: String,
    started_at: DateTime<Utc>,
}

/// A login that came back with the right state: what to finish it with.
pub struct Resumed {
    pub verifier: String,
    pub next: String,
}

/// Unfinished logins, in process memory: a restart only cancels logins in flight.
pub struct DiscordLogin {
    pub client: DiscordClient,
    pub api: Arc<dyn DiscordApi>,
    pending: Mutex<HashMap<String, Pending>>,
}

pub struct Started {
    /// Value of the pre-auth cookie.
    pub login_id: String,
    pub authorize_url: String,
}

impl DiscordLogin {
    pub fn new(client: DiscordClient, api: Arc<dyn DiscordApi>) -> Self {
        Self {
            client,
            api,
            pending: Mutex::new(HashMap::new()),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn begin(&self, next: String, now: DateTime<Utc>) -> Option<Started> {
        let login_id = crypto::random_token()?;
        let state = crypto::random_token()?;
        let verifier = crypto::random_token()?;
        let challenge = crypto::sha256_base64url(verifier.as_bytes());
        {
            let mut pending = self.pending();
            pending.retain(|_, login| now - login.started_at < LOGIN_TTL);
            if pending.len() >= MAX_PENDING
                && let Some(oldest) = pending
                    .iter()
                    .min_by_key(|(_, login)| login.started_at)
                    .map(|(key, _)| key.clone())
            {
                pending.remove(&oldest);
            }
            pending.insert(
                crypto::sha256_hex(login_id.as_bytes()),
                Pending {
                    state_hash: crypto::sha256_hex(state.as_bytes()),
                    verifier,
                    next,
                    started_at: now,
                },
            );
        }
        let authorize_url = format!(
            "{AUTHORIZE_URL}?{}",
            wire::form(&[
                ("response_type", "code"),
                ("client_id", &self.client.client_id),
                ("scope", "identify"),
                ("state", &state),
                ("redirect_uri", &self.client.redirect_uri),
                ("code_challenge", &challenge),
                ("code_challenge_method", "S256"),
            ])
        );
        Some(Started {
            login_id,
            authorize_url,
        })
    }

    /// One-time: the pending login is removed whether or not `state` matches.
    pub fn resume(&self, login_id: &str, state: &str, now: DateTime<Utc>) -> Option<Resumed> {
        let pending = self
            .pending()
            .remove(&crypto::sha256_hex(login_id.as_bytes()))?;
        let fresh = now - pending.started_at < LOGIN_TTL;
        let state_ok = crypto::constant_eq(
            crypto::sha256_hex(state.as_bytes()).as_bytes(),
            pending.state_hash.as_bytes(),
        );
        (fresh && state_ok).then_some(Resumed {
            verifier: pending.verifier,
            next: pending.next,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Unused;

    impl DiscordApi for Unused {
        fn exchange_code<'a>(
            &'a self,
            _: CodeExchange<'a>,
        ) -> DiscordFuture<'a, Result<AccessToken, DiscordError>> {
            unreachable!()
        }
        fn current_user<'a>(
            &'a self,
            _: &'a AccessToken,
        ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>> {
            unreachable!()
        }
        fn revoke<'a>(&'a self, _: &'a DiscordClient, _: AccessToken) -> DiscordFuture<'a, ()> {
            unreachable!()
        }
    }

    fn login() -> DiscordLogin {
        DiscordLogin::new(
            DiscordClient {
                client_id: "123".into(),
                client_secret: Secret::new("shh"),
                redirect_uri: "https://kanade.test/api/admin/auth/discord/callback".into(),
            },
            Arc::new(Unused),
        )
    }

    fn param(url: &str, key: &str) -> String {
        let pairs = wire::query_pairs(url.split_once('?').map(|(_, query)| query));
        wire::query_value(&pairs, key).unwrap()
    }

    #[test]
    fn authorize_url_carries_identify_state_and_s256_challenge_only() {
        let now = DateTime::UNIX_EPOCH + TimeDelta::days(20_000);
        let login = login();
        let started = login.begin("/week".into(), now).unwrap();
        let url = &started.authorize_url;
        assert!(url.starts_with("https://discord.com/oauth2/authorize?"));
        assert_eq!(param(url, "scope"), "identify");
        assert_eq!(param(url, "response_type"), "code");
        assert_eq!(param(url, "code_challenge_method"), "S256");
        assert_eq!(
            param(url, "redirect_uri"),
            "https://kanade.test/api/admin/auth/discord/callback"
        );
        assert!(
            !url.contains("shh"),
            "the client secret never leaves the server"
        );
        let state = param(url, "state");
        let resumed = login.resume(&started.login_id, &state, now).unwrap();
        assert_eq!(
            crypto::sha256_base64url(resumed.verifier.as_bytes()),
            param(url, "code_challenge")
        );
        assert_eq!(resumed.next, "/week");
        assert!(
            login.resume(&started.login_id, &state, now).is_none(),
            "one-time"
        );
    }

    #[test]
    fn wrong_state_or_stale_login_is_refused_and_consumed() {
        let now = DateTime::UNIX_EPOCH + TimeDelta::days(20_000);
        let login = login();
        let started = login.begin("/".into(), now).unwrap();
        let state = param(&started.authorize_url, "state");
        assert!(login.resume(&started.login_id, "forged", now).is_none());
        assert!(login.resume(&started.login_id, &state, now).is_none());

        let late = login.begin("/".into(), now).unwrap();
        let state = param(&late.authorize_url, "state");
        assert!(
            login
                .resume(&late.login_id, &state, now + LOGIN_TTL)
                .is_none()
        );
    }
}
