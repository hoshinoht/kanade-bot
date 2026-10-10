//! Discord OAuth2 authorization-code flow with PKCE S256 and scope `identify`.
//! Discord endpoints: https://docs.discord.com/developers/topics/oauth2 (token
//! and revocation take form bodies with HTTP Basic client credentials);
//! PKCE parameters: https://docs.discord.food/topics/oauth2#pkce (S256 only).
//! The access token is used for one `/users/@me` call, then revoked.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    net::IpAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, TimeDelta, Utc};

use super::{crypto, wire};

pub const AUTHORIZE_URL: &str = "https://discord.com/oauth2/authorize";
/// The admin origin's callback.
pub const CALLBACK_PATH: &str = "/api/admin/auth/discord/callback";
/// The public origin's callback (the member realm's own application, D2-A).
pub const PUBLIC_CALLBACK_PATH: &str = "/api/public/auth/discord/callback";
/// How long a started login may take to come back.
pub const LOGIN_TTL: TimeDelta = TimeDelta::minutes(10);
/// Bounds memory held for unfinished logins; when full, new logins are refused
/// rather than evicting anyone else's.
pub const MAX_PENDING: usize = 256;
/// One client can hold this many unfinished logins; its oldest yields first.
pub const MAX_PENDING_PER_CLIENT: usize = 5;
/// Longest Discord `retry_after` honoured.
const MAX_COOLDOWN: Duration = Duration::from_secs(3600);

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

/// The token reply: the token and the scope Discord actually granted.
#[derive(Debug)]
pub struct TokenGrant {
    pub token: AccessToken,
    pub scope: String,
}

impl TokenGrant {
    /// Exactly `identify`, nothing broader.
    pub fn identify_only(&self) -> bool {
        let mut scopes = self.scope.split_whitespace();
        scopes.next() == Some("identify") && scopes.all(|scope| scope == "identify")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordUser {
    pub id: String,
    pub username: String,
    pub global_name: Option<String>,
    pub bot: bool,
    /// The user avatar hash (`identify` returns it), already validated.
    pub avatar: Option<String>,
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
    /// 429 with its `retry_after`.
    RateLimited(Duration),
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
    ) -> DiscordFuture<'a, Result<TokenGrant, DiscordError>>;

    fn current_user<'a>(
        &'a self,
        token: &'a AccessToken,
    ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>>;

    fn revoke<'a>(
        &'a self,
        client: &'a DiscordClient,
        token: AccessToken,
    ) -> DiscordFuture<'a, Result<(), DiscordError>>;
}

struct Pending {
    state_hash: String,
    verifier: String,
    next: String,
    started_at: DateTime<Utc>,
    client: Option<IpAddr>,
}

/// A login that came back with the right state: what to finish it with.
pub struct Resumed {
    pub verifier: String,
    pub next: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginError {
    /// Too many unfinished logins overall.
    Busy,
    /// No system randomness.
    Unavailable,
}

/// Unfinished logins, in process memory: a restart only cancels logins in flight.
/// Each realm has its own instance, so its own pending caps and cooldown.
pub struct DiscordLogin {
    pub client: DiscordClient,
    pub api: Arc<dyn DiscordApi>,
    /// [`AUTHORIZE_URL`] except in the browser test fixture's fake Discord.
    authorize_url: String,
    /// Ask Discord with `prompt=none` (skip consent for an app already authorized).
    prompt_none: bool,
    pending: Mutex<HashMap<String, Pending>>,
    cooldown_until: Mutex<Option<DateTime<Utc>>>,
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
            authorize_url: AUTHORIZE_URL.into(),
            prompt_none: false,
            pending: Mutex::new(HashMap::new()),
            cooldown_until: Mutex::new(None),
        }
    }

    /// Send `prompt=none` with every authorization request.
    pub fn with_prompt_none(mut self) -> Self {
        self.prompt_none = true;
        self
    }

    /// A stand-in authorization page (the browser test fixture's fake Discord).
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_authorize_url(mut self, url: impl Into<String>) -> Self {
        self.authorize_url = url.into();
        self
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn cooldown(&self) -> std::sync::MutexGuard<'_, Option<DateTime<Utc>>> {
        self.cooldown_until
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Discord asked us to back off: no new Discord calls until then.
    pub fn cool_down(&self, now: DateTime<Utc>, retry_after: Duration) {
        let wait = retry_after.clamp(Duration::from_secs(1), MAX_COOLDOWN);
        let until = now + TimeDelta::from_std(wait).unwrap_or(TimeDelta::hours(1));
        let mut cooldown = self.cooldown();
        if cooldown.is_none_or(|current| current < until) {
            *cooldown = Some(until);
        }
    }

    pub fn cooling_down(&self, now: DateTime<Utc>) -> bool {
        self.cooldown().is_some_and(|until| now < until)
    }

    pub fn begin(
        &self,
        next: String,
        now: DateTime<Utc>,
        client: Option<IpAddr>,
    ) -> Result<Started, BeginError> {
        let token = || crypto::random_token().ok_or(BeginError::Unavailable);
        let (login_id, state, verifier) = (token()?, token()?, token()?);
        let challenge = crypto::sha256_base64url(verifier.as_bytes());
        {
            let mut pending = self.pending();
            pending.retain(|_, login| now - login.started_at < LOGIN_TTL);
            let mut own: Vec<_> = pending
                .iter()
                .filter(|(_, login)| login.client == client)
                .map(|(key, login)| (login.started_at, key.clone()))
                .collect();
            if own.len() >= MAX_PENDING_PER_CLIENT {
                own.sort();
                for (_, key) in &own[..=own.len() - MAX_PENDING_PER_CLIENT] {
                    pending.remove(key);
                }
            } else if pending.len() >= MAX_PENDING {
                return Err(BeginError::Busy);
            }
            pending.insert(
                crypto::sha256_hex(login_id.as_bytes()),
                Pending {
                    state_hash: crypto::sha256_hex(state.as_bytes()),
                    verifier,
                    next,
                    started_at: now,
                    client,
                },
            );
        }
        let mut params = vec![
            ("response_type", "code"),
            ("client_id", self.client.client_id.as_str()),
            ("scope", "identify"),
            ("state", state.as_str()),
            ("redirect_uri", self.client.redirect_uri.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
        ];
        if self.prompt_none {
            params.push(("prompt", "none"));
        }
        let authorize_url = format!("{}?{}", self.authorize_url, wire::form(&params));
        Ok(Started {
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
        ) -> DiscordFuture<'a, Result<TokenGrant, DiscordError>> {
            unreachable!()
        }
        fn current_user<'a>(
            &'a self,
            _: &'a AccessToken,
        ) -> DiscordFuture<'a, Result<DiscordUser, DiscordError>> {
            unreachable!()
        }
        fn revoke<'a>(
            &'a self,
            _: &'a DiscordClient,
            _: AccessToken,
        ) -> DiscordFuture<'a, Result<(), DiscordError>> {
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

    fn now() -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + TimeDelta::days(20_000)
    }

    #[test]
    fn authorize_url_carries_identify_state_and_s256_challenge_only() {
        let login = login();
        let started = login.begin("/week".into(), now(), None).unwrap();
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
        let resumed = login.resume(&started.login_id, &state, now()).unwrap();
        assert_eq!(
            crypto::sha256_base64url(resumed.verifier.as_bytes()),
            param(url, "code_challenge")
        );
        assert_eq!(resumed.next, "/week");
        assert!(
            login.resume(&started.login_id, &state, now()).is_none(),
            "one-time"
        );
    }

    #[test]
    fn only_a_login_built_with_prompt_none_sends_it() {
        let admin = login().begin("/".into(), now(), None).unwrap();
        let pairs = wire::query_pairs(admin.authorize_url.split_once('?').map(|(_, q)| q));
        assert_eq!(wire::query_value(&pairs, "prompt"), None);
        let member = login().with_prompt_none();
        let started = member.begin("/".into(), now(), None).unwrap();
        assert_eq!(param(&started.authorize_url, "prompt"), "none");
    }

    #[test]
    fn wrong_state_or_stale_login_is_refused_and_consumed() {
        let login = login();
        let started = login.begin("/".into(), now(), None).unwrap();
        let state = param(&started.authorize_url, "state");
        assert!(login.resume(&started.login_id, "forged", now()).is_none());
        assert!(login.resume(&started.login_id, &state, now()).is_none());

        let late = login.begin("/".into(), now(), None).unwrap();
        let state = param(&late.authorize_url, "state");
        assert!(
            login
                .resume(&late.login_id, &state, now() + LOGIN_TTL)
                .is_none()
        );
    }

    #[test]
    fn one_client_cannot_evict_other_clients_pending_logins() {
        let login = login();
        let victim: Option<IpAddr> = Some([10, 0, 0, 9].into());
        let attacker: Option<IpAddr> = Some([10, 0, 0, 66].into());
        let kept = login.begin("/".into(), now(), victim).unwrap();
        let state = param(&kept.authorize_url, "state");
        for i in 0..(MAX_PENDING * 2) {
            let at = now() + TimeDelta::milliseconds(i as i64);
            login.begin("/".into(), at, attacker).unwrap();
        }
        assert_eq!(login.pending().len(), 1 + MAX_PENDING_PER_CLIENT);
        assert!(login.resume(&kept.login_id, &state, now()).is_some());

        // Many distinct clients fill the table: new logins are refused, none evicted.
        let full = super::DiscordLogin::new(login.client.clone(), Arc::new(Unused));
        let first = full
            .begin("/".into(), now(), Some([10, 1, 0, 0].into()))
            .unwrap();
        for i in 1..MAX_PENDING {
            let client = Some(IpAddr::from([10, 1, (i / 256) as u8, (i % 256) as u8]));
            full.begin("/".into(), now(), client).unwrap();
        }
        assert_eq!(
            full.begin("/".into(), now(), Some([10, 2, 0, 0].into()))
                .err(),
            Some(BeginError::Busy)
        );
        let state = param(&first.authorize_url, "state");
        assert!(full.resume(&first.login_id, &state, now()).is_some());
    }

    #[test]
    fn cooldowns_hold_until_retry_after_and_are_bounded() {
        let login = login();
        assert!(!login.cooling_down(now()));
        login.cool_down(now(), Duration::from_secs(30));
        assert!(login.cooling_down(now() + TimeDelta::seconds(29)));
        assert!(!login.cooling_down(now() + TimeDelta::seconds(30)));
        login.cool_down(now(), Duration::from_secs(10 * 3600));
        assert!(!login.cooling_down(now() + TimeDelta::hours(1)));
    }

    #[test]
    fn only_exactly_identify_is_accepted() {
        let grant = |scope: &str| TokenGrant {
            token: AccessToken::new("t"),
            scope: scope.into(),
        };
        assert!(grant("identify").identify_only());
        for scope in [
            "",
            "identify email",
            "email",
            "guilds identify",
            "identify guilds.join",
        ] {
            assert!(!grant(scope).identify_only(), "{scope:?}");
        }
    }
}
