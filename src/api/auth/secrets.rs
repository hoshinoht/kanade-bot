//! Production construction: secrets come from files (Compose secrets), never
//! from environment values, and errors never quote file contents.

use std::{path::Path, sync::Arc};

use chrono::TimeDelta;

use super::{
    AdminAuth, SessionPolicy,
    discord::{DiscordClient, DiscordLogin, Secret},
    discord_http::HttpsDiscord,
    staff::StaffGate,
};
use crate::{
    infrastructure::store::web_sessions::WebSessionStore,
    runtime::{config::AdminAuthSettings, error::Error},
};

const MAX_SECRET_BYTES: u64 = 4096;
/// Break-glass tokens must be long enough that guessing is not a strategy.
const MIN_TOKEN_BYTES: usize = 32;

fn read_secret(path: &Path, variable: &str) -> Result<String, Error> {
    let refused = || Error::Configuration(format!("{variable} must name a readable secret file"));
    let metadata = std::fs::metadata(path).map_err(|_| refused())?;
    if !metadata.is_file() || metadata.len() > MAX_SECRET_BYTES {
        return Err(refused());
    }
    let text = std::fs::read_to_string(path).map_err(|_| refused())?;
    let secret = text.trim_end_matches(['\r', '\n']).to_owned();
    if secret.is_empty() || secret.chars().any(char::is_control) {
        return Err(Error::Configuration(format!(
            "{variable} must contain one non-empty line"
        )));
    }
    Ok(secret)
}

pub fn from_settings(
    settings: &AdminAuthSettings,
    sessions: Arc<dyn WebSessionStore>,
    staff: Arc<dyn StaffGate>,
) -> Result<AdminAuth, Error> {
    let to_delta = |duration: std::time::Duration| {
        TimeDelta::from_std(duration)
            .map_err(|_| Error::Configuration("session timeouts are out of range".into()))
    };
    let mut auth = AdminAuth::new(sessions, staff)
        .with_policy(SessionPolicy {
            idle: to_delta(settings.session_idle)?,
            absolute: to_delta(settings.session_absolute)?,
            ..SessionPolicy::default()
        })
        .with_tailscale_logins(settings.tailscale_logins.iter().cloned());
    if let Some(discord) = &settings.discord {
        let secret = read_secret(
            &discord.client_secret_file,
            "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE",
        )?;
        let api = HttpsDiscord::new().ok_or_else(|| {
            Error::Startup("the Rustls ring provider must be installed before sign-in".into())
        })?;
        auth = auth.with_discord(DiscordLogin::new(
            DiscordClient {
                client_id: discord.client_id.clone(),
                client_secret: Secret::new(secret),
                redirect_uri: discord.redirect_uri.clone(),
            },
            Arc::new(api),
        ));
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
}
