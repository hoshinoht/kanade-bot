//! Production construction: secrets come from the secrets directory, never
//! from environment values, and errors never quote file contents.

use std::sync::Arc;

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
        secrets::SecretFile,
    },
};

/// Break-glass tokens must be long enough that guessing is not a strategy.
const MIN_TOKEN_BYTES: usize = 32;

/// The secret the edge sends in `X-Kanade-Edge-Auth`, sealed for
/// constant-time checks; `None` while the file is absent, unless
/// `required` says why it is needed.
pub fn edge_secret(
    file: &SecretFile,
    required: Option<&str>,
) -> Result<Option<SealedSecret>, Error> {
    let secret = match required {
        Some(why) => file.read(why)?,
        None => match file.read_optional()? {
            Some(secret) => secret,
            None => return Ok(None),
        },
    };
    long_enough(file, secret.expose())?;
    SealedSecret::new(secret.expose().as_bytes())
        .map(Some)
        .ok_or_else(|| Error::Startup("system randomness is unavailable".into()))
}

fn long_enough(file: &SecretFile, secret: &str) -> Result<(), Error> {
    if secret.len() < MIN_TOKEN_BYTES {
        return Err(Error::Configuration(format!(
            "{} must hold at least {MIN_TOKEN_BYTES} bytes",
            file.label()
        )));
    }
    Ok(())
}

fn to_delta(duration: std::time::Duration) -> Result<TimeDelta, Error> {
    TimeDelta::from_std(duration)
        .map_err(|_| Error::Configuration("session timeouts are out of range".into()))
}

/// A Discord login over HTTPS with the client secret read from its file,
/// which `why` needs.
fn discord_login(settings: &DiscordOAuthSettings, why: &str) -> Result<DiscordLogin, Error> {
    let secret = settings.client_secret.read(why)?;
    let api = HttpsDiscord::new().ok_or_else(|| {
        Error::Startup("the Rustls ring provider must be installed before sign-in".into())
    })?;
    Ok(DiscordLogin::new(
        DiscordClient {
            client_id: settings.client_id.clone(),
            client_secret: Secret::new(secret.expose().to_owned()),
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
            "kanade.toml `admin.discord_client_id` turns on admin Discord sign-in",
        )?);
    }
    // Break-glass sign-in exists only while the file does.
    if let Some(token) = settings.token.read_optional()? {
        long_enough(&settings.token, token.expose())?;
        auth = auth
            .with_breakglass(token.expose().as_bytes())
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
            "kanade.toml `public.discord_client_id` turns on member sign-in",
        )?);
    }
    Ok(auth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::auth::staff::{GateFuture, StaffCheck};
    use crate::infrastructure::store::SqliteStore;

    struct Nobody;

    impl StaffGate for Nobody {
        fn check<'a>(&'a self, _: &'a str) -> GateFuture<'a, StaffCheck> {
            Box::pin(async { StaffCheck::NotStaff })
        }
    }

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("kanade-auth-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self, name: &'static str) -> SecretFile {
            SecretFile::new(&self.0, name)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    async fn build(dir: &Dir, token: Option<&str>) -> Result<AdminAuth, Error> {
        let file = dir.file(crate::runtime::secrets::ADMIN_TOKEN);
        if let Some(token) = token {
            std::fs::write(file.path(), token).unwrap();
        }
        let settings = AdminAuthSettings {
            token: file,
            ..AdminAuthSettings::default()
        };
        from_settings(
            &settings,
            Arc::new(SqliteStore::open_temp().await),
            Arc::new(Nobody),
        )
    }

    #[tokio::test]
    async fn token_files_are_trimmed_bounded_optional_and_never_quoted() {
        let dir = Dir::new();
        let token = "x".repeat(40);
        let auth = build(&dir, Some(&format!("{token}\n"))).await.unwrap();
        assert!(auth.breakglass_matches(token.as_bytes()).is_some());
        assert!(!format!("{auth:?}").contains(&token));

        let error = build(&dir, Some("short-secret-value"))
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "secrets file `admin_token` must hold at least 32 bytes"
        );
        let error = build(&dir, Some("")).await.unwrap_err().to_string();
        assert!(!error.contains("short"), "{error}");

        // Absent: no break-glass sign-in, and no refusal.
        std::fs::remove_file(dir.file(crate::runtime::secrets::ADMIN_TOKEN).path()).unwrap();
        let auth = build(&dir, None).await.unwrap();
        assert!(auth.breakglass_matches(token.as_bytes()).is_none());
    }

    #[test]
    fn edge_secrets_are_long_sealed_and_required_only_when_asked() {
        let dir = Dir::new();
        let file = dir.file(crate::runtime::secrets::EDGE_SECRET);
        assert!(edge_secret(&file, None).unwrap().is_none());
        assert_eq!(
            edge_secret(&file, Some("logins need it"))
                .unwrap_err()
                .to_string(),
            format!(
                "secrets file `edge_secret` is missing from {}: logins need it",
                dir.0.display()
            )
        );
        std::fs::write(file.path(), "too-short\n").unwrap();
        let error = edge_secret(&file, None).unwrap_err().to_string();
        assert_eq!(
            error,
            "secrets file `edge_secret` must hold at least 32 bytes"
        );
        let secret = "e".repeat(48);
        std::fs::write(file.path(), format!("{secret}\n")).unwrap();
        let sealed = edge_secret(&file, None).unwrap().unwrap();
        assert!(sealed.matches(secret.as_bytes()));
        assert!(!sealed.matches(b"e"));
    }
}
