//! Production construction: secrets come from files (Compose secrets), never
//! from environment values, and errors never quote file contents.

use std::{path::Path, sync::Arc};

use chrono::TimeDelta;

use super::{
    AdminAuth, SessionPolicy,
    crypto::SealedSecret,
    discord::{DiscordClient, DiscordLogin, Secret},
    discord_http::HttpsDiscord,
    member::{EligibilityGate, MemberAuth, MemberPolicy, PortalOpen},
    staff::StaffGate,
};
use crate::{
    infrastructure::store::web_sessions::WebSessionStore,
    runtime::{
        config::{AdminAuthSettings, DiscordOAuthSettings, PublicAuthSettings},
        error::Error,
        secrets::read_secret,
    },
};

/// Break-glass tokens must be long enough that guessing is not a strategy.
const MIN_TOKEN_BYTES: usize = 32;

/// The secret the edge sends in `X-Kanade-Edge-Auth`, sealed for constant-time checks.
pub fn edge_secret(path: &Path) -> Result<SealedSecret, Error> {
    let secret = read_secret(path, "KANADE_EDGE_SECRET_FILE")?;
    if secret.len() < MIN_TOKEN_BYTES {
        return Err(Error::Configuration(format!(
            "KANADE_EDGE_SECRET_FILE must hold at least {MIN_TOKEN_BYTES} bytes"
        )));
    }
    SealedSecret::new(secret.as_bytes())
        .ok_or_else(|| Error::Startup("system randomness is unavailable".into()))
}

fn to_delta(duration: std::time::Duration) -> Result<TimeDelta, Error> {
    TimeDelta::from_std(duration)
        .map_err(|_| Error::Configuration("session timeouts are out of range".into()))
}

/// A Discord login over HTTPS with the client secret read from its file.
fn discord_login(settings: &DiscordOAuthSettings, secret_key: &str) -> Result<DiscordLogin, Error> {
    let secret = read_secret(&settings.client_secret_file, secret_key)?;
    let api = HttpsDiscord::new().ok_or_else(|| {
        Error::Startup("the Rustls ring provider must be installed before sign-in".into())
    })?;
    Ok(DiscordLogin::new(
        DiscordClient {
            client_id: settings.client_id.clone(),
            client_secret: Secret::new(secret),
            redirect_uri: settings.redirect_uri.clone(),
        },
        Arc::new(api),
    ))
}

pub fn from_settings(
    settings: &AdminAuthSettings,
    sessions: Arc<dyn WebSessionStore>,
    staff: Arc<dyn StaffGate>,
) -> Result<AdminAuth, Error> {
    let mut auth = AdminAuth::new(sessions, staff)
        .with_policy(SessionPolicy {
            idle: to_delta(settings.session_idle)?,
            absolute: to_delta(settings.session_absolute)?,
            ..SessionPolicy::default()
        })
        .with_tailscale_logins(settings.tailscale_logins.iter().cloned());
    if let Some(discord) = &settings.discord {
        auth = auth.with_discord(discord_login(
            discord,
            "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE",
        )?);
    }
    if let Some(path) = &settings.token_file {
        let token = read_secret(path, "KANADE_ADMIN_TOKEN_FILE")?;
        if token.len() < MIN_TOKEN_BYTES {
            return Err(Error::Configuration(format!(
                "KANADE_ADMIN_TOKEN_FILE must hold at least {MIN_TOKEN_BYTES} bytes"
            )));
        }
        auth = auth
            .with_breakglass(token.as_bytes())
            .ok_or_else(|| Error::Startup("system randomness is unavailable".into()))?;
    }
    Ok(auth)
}

/// The member realm of the public origin: its own Discord application (when
/// configured) and the `[public]` lifetimes; the re-check, grace and cap
/// stay fixed.
pub fn member_from_settings(
    settings: &PublicAuthSettings,
    sessions: Arc<dyn WebSessionStore>,
    gate: Arc<dyn EligibilityGate>,
    open: PortalOpen,
) -> Result<MemberAuth, Error> {
    let mut auth = MemberAuth::new(sessions, gate, open).with_policy(MemberPolicy {
        idle: to_delta(settings.session_idle)?,
        absolute: to_delta(settings.session_absolute)?,
        fresh: to_delta(settings.fresh_write)?,
        ..MemberPolicy::default()
    });
    if let Some(discord) = &settings.discord {
        auth = auth.with_discord(discord_login(
            discord,
            "KANADE_PUBLIC_DISCORD_CLIENT_SECRET_FILE",
        )?);
    }
    Ok(auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::staff::{GateFuture, StaffCheck};
    use crate::infrastructure::store::MemoryScheduleStore;

    struct Nobody;

    impl StaffGate for Nobody {
        fn check<'a>(&'a self, _: &'a str) -> GateFuture<'a, StaffCheck> {
            Box::pin(async { StaffCheck::NotStaff })
        }
    }

    fn build(token: &str) -> Result<AdminAuth, Error> {
        let path = std::env::temp_dir().join(format!("kanade-token-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, token).unwrap();
        let settings = AdminAuthSettings {
            token_file: Some(path.clone()),
            ..AdminAuthSettings::default()
        };
        let result = from_settings(
            &settings,
            Arc::new(MemoryScheduleStore::new()),
            Arc::new(Nobody),
        );
        std::fs::remove_file(path).unwrap();
        result
    }

    #[test]
    fn token_files_are_trimmed_bounded_and_never_quoted() {
        let token = "x".repeat(40);
        let auth = build(&format!("{token}\n")).unwrap();
        assert!(auth.breakglass_matches(token.as_bytes()).is_some());
        assert!(!format!("{auth:?}").contains(&token));

        let error = build("short-secret-value").unwrap_err().to_string();
        assert_eq!(error, "KANADE_ADMIN_TOKEN_FILE must hold at least 32 bytes");
        let error = build("").unwrap_err().to_string();
        assert!(!error.contains("short"), "{error}");

        let missing = AdminAuthSettings {
            token_file: Some("/nonexistent/kanade/token".into()),
            ..AdminAuthSettings::default()
        };
        let error = from_settings(
            &missing,
            Arc::new(MemoryScheduleStore::new()),
            Arc::new(Nobody),
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "KANADE_ADMIN_TOKEN_FILE must name a readable secret file"
        );
    }

    #[test]
    fn edge_secrets_are_long_and_sealed() {
        let path = std::env::temp_dir().join(format!("kanade-edge-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "too-short\n").unwrap();
        let error = edge_secret(&path).unwrap_err().to_string();
        assert_eq!(error, "KANADE_EDGE_SECRET_FILE must hold at least 32 bytes");
        let secret = "e".repeat(48);
        std::fs::write(&path, format!("{secret}\n")).unwrap();
        let sealed = edge_secret(&path).unwrap();
        assert!(sealed.matches(secret.as_bytes()));
        assert!(!sealed.matches(b"e"));
        std::fs::remove_file(path).unwrap();
    }
}
