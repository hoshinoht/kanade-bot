//! Member sign-in settings for the public origin (`[public]`). The portal
//! opens with the live admin switch `self_service.public_portal` (D7-B); there
//! is no open key. Secrets are file paths only, as for admin.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use super::{DiscordOAuthSettings, Error, HttpConfig, non_empty, parse_bounded_u64};

/// The only callback path the public origin serves.
const CALLBACK_PATH: &str = "/api/public/auth/discord/callback";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicAuthSettings {
    /// The public origin's own Discord application (D2-A); `None` keeps the
    /// portal closed whatever the switch says.
    pub discord: Option<DiscordOAuthSettings>,
    pub session_idle: Duration,
    pub session_absolute: Duration,
    /// How long after a Discord round trip writes count as fresh.
    pub fresh_write: Duration,
}

impl Default for PublicAuthSettings {
    fn default() -> Self {
        Self {
            discord: None,
            session_idle: Duration::from_secs(30 * 60),
            session_absolute: Duration::from_secs(8 * 60 * 60),
            fresh_write: Duration::from_secs(15 * 60),
        }
    }
}

impl PublicAuthSettings {
    pub(super) fn from_mapping(
        values: &BTreeMap<String, String>,
        http: &HttpConfig,
    ) -> Result<Self, Error> {
        let plain = "KANADE_PUBLIC_DISCORD_CLIENT_SECRET";
        if non_empty(values, plain).is_some() {
            return Err(Error::Configuration(format!(
                "{plain} is not read; use {plain}_FILE"
            )));
        }
        let idle = parse_bounded_u64(values, "KANADE_PUBLIC_SESSION_IDLE_MINUTES", 30, 5, 60)?;
        let absolute = parse_bounded_u64(values, "KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS", 8, 1, 24)?;
        let fresh = parse_bounded_u64(values, "KANADE_PUBLIC_FRESH_WRITE_MINUTES", 15, 5, 30)?;
        if fresh > idle {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_FRESH_WRITE_MINUTES must not exceed KANADE_PUBLIC_SESSION_IDLE_MINUTES"
                    .into(),
            ));
        }
        if idle > absolute * 60 {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_SESSION_IDLE_MINUTES must not exceed the absolute lifetime".into(),
            ));
        }
        Ok(Self {
            discord: discord(values, http)?,
            session_idle: Duration::from_secs(idle * 60),
            session_absolute: Duration::from_secs(absolute * 60 * 60),
            fresh_write: Duration::from_secs(fresh * 60),
        })
    }
}

fn discord(
    values: &BTreeMap<String, String>,
    http: &HttpConfig,
) -> Result<Option<DiscordOAuthSettings>, Error> {
    let parts = [
        non_empty(values, "KANADE_PUBLIC_DISCORD_CLIENT_ID"),
        non_empty(values, "KANADE_PUBLIC_DISCORD_CLIENT_SECRET_FILE"),
        non_empty(values, "KANADE_PUBLIC_DISCORD_REDIRECT_URI"),
    ];
    let [Some(client_id), Some(secret_file), Some(redirect_uri)] = parts else {
        if parts.iter().any(Option::is_some) {
            return Err(Error::Configuration(
                "member sign-in needs KANADE_PUBLIC_DISCORD_CLIENT_ID, \
                 KANADE_PUBLIC_DISCORD_CLIENT_SECRET_FILE and KANADE_PUBLIC_DISCORD_REDIRECT_URI"
                    .into(),
            ));
        }
        return Ok(None);
    };
    if client_id.len() > 20 || !client_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Configuration(
            "KANADE_PUBLIC_DISCORD_CLIENT_ID must be a Discord application id".into(),
        ));
    }
    let public_host = http.public_host.as_deref().ok_or_else(|| {
        Error::Configuration("KANADE_PUBLIC_HOST is required for member sign-in".into())
    })?;
    // Exact, and https only: the public origin is never a loopback dev host.
    if redirect_uri != format!("https://{public_host}{CALLBACK_PATH}") {
        return Err(Error::Configuration(format!(
            "KANADE_PUBLIC_DISCORD_REDIRECT_URI must be https://KANADE_PUBLIC_HOST{CALLBACK_PATH}"
        )));
    }
    Ok(Some(DiscordOAuthSettings {
        client_id: client_id.to_owned(),
        client_secret_file: PathBuf::from(secret_file),
        redirect_uri: redirect_uri.to_owned(),
    }))
}

#[cfg(test)]
mod tests {
    use super::super::RuntimeConfig;

    fn config(pairs: &[(&str, &str)]) -> Result<RuntimeConfig, String> {
        let mut values = std::collections::BTreeMap::from([(
            "KANADE_TIMEZONE".to_owned(),
            "Asia/Kuala_Lumpur".to_owned(),
        )]);
        for (key, value) in pairs {
            values.insert((*key).into(), (*value).into());
        }
        RuntimeConfig::from_mapping(&values).map_err(|error| error.to_string())
    }

    const DISCORD: [(&str, &str); 4] = [
        ("KANADE_PUBLIC_HOST", "kanade-pub.example"),
        ("KANADE_PUBLIC_DISCORD_CLIENT_ID", "1234567890"),
        (
            "KANADE_PUBLIC_DISCORD_CLIENT_SECRET_FILE",
            "/run/secrets/public_discord_client_secret",
        ),
        (
            "KANADE_PUBLIC_DISCORD_REDIRECT_URI",
            "https://kanade-pub.example/api/public/auth/discord/callback",
        ),
    ];

    #[test]
    fn defaults_keep_the_portal_closed_with_bounded_sessions() {
        let auth = config(&[]).unwrap().public_auth;
        assert_eq!(auth, super::PublicAuthSettings::default());
        assert_eq!(auth.discord, None);
        assert_eq!(
            (
                auth.session_idle.as_secs(),
                auth.session_absolute.as_secs(),
                auth.fresh_write.as_secs()
            ),
            (30 * 60, 8 * 3600, 15 * 60)
        );
    }

    #[test]
    fn lifetimes_are_bounded_and_ordered() {
        for (key, low, high) in [
            ("KANADE_PUBLIC_SESSION_IDLE_MINUTES", "4", "61"),
            ("KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS", "0", "25"),
            ("KANADE_PUBLIC_FRESH_WRITE_MINUTES", "4", "31"),
        ] {
            for value in [low, high] {
                let error = config(&[(key, value)]).unwrap_err();
                assert!(error.contains("must be between"), "{key}={value}: {error}");
            }
        }
        assert!(
            config(&[
                ("KANADE_PUBLIC_SESSION_IDLE_MINUTES", "10"),
                ("KANADE_PUBLIC_FRESH_WRITE_MINUTES", "20"),
            ])
            .unwrap_err()
            .contains("must not exceed")
        );
        let auth = config(&[
            ("KANADE_PUBLIC_SESSION_IDLE_MINUTES", "60"),
            ("KANADE_PUBLIC_SESSION_ABSOLUTE_HOURS", "1"),
            ("KANADE_PUBLIC_FRESH_WRITE_MINUTES", "30"),
        ])
        .unwrap()
        .public_auth;
        assert_eq!(auth.session_idle.as_secs(), 3600);
        assert_eq!(auth.fresh_write.as_secs(), 1800);
    }

    #[test]
    fn discord_needs_all_parts_the_public_host_and_the_exact_callback() {
        let auth = config(&DISCORD).unwrap().public_auth;
        assert_eq!(auth.discord.unwrap().client_id, "1234567890");
        assert!(config(&DISCORD[..3]).unwrap_err().contains("needs"));
        assert!(
            config(&DISCORD[1..])
                .unwrap_err()
                .contains("KANADE_PUBLIC_HOST is required")
        );
        for bad in [
            "https://evil.example/api/public/auth/discord/callback",
            "http://kanade-pub.example/api/public/auth/discord/callback",
            "https://kanade-pub.example/api/admin/auth/discord/callback",
            "https://kanade-pub.example/api/public/auth/discord/callback?x=1",
            "https://kanade-pub.example/api/public/auth/discord/callback/",
        ] {
            let mut pairs = DISCORD.to_vec();
            pairs[3].1 = bad;
            let error = config(&pairs).unwrap_err();
            assert!(error.contains("must be https://"), "{bad}");
            assert!(!error.contains("evil"));
        }
        let mut local = DISCORD.to_vec();
        local[0].1 = "localhost:4394";
        local[3].1 = "http://localhost:4394/api/public/auth/discord/callback";
        assert!(
            config(&local).is_err(),
            "no http exception on the public origin"
        );
        let mut bad_id = DISCORD.to_vec();
        bad_id[1].1 = "not-a-snowflake";
        assert!(config(&bad_id).unwrap_err().contains("application id"));
    }

    #[test]
    fn a_plain_public_secret_is_refused() {
        assert_eq!(
            config(&[("KANADE_PUBLIC_DISCORD_CLIENT_SECRET", "hunter2")]).unwrap_err(),
            "KANADE_PUBLIC_DISCORD_CLIENT_SECRET is not read; use KANADE_PUBLIC_DISCORD_CLIENT_SECRET_FILE"
        );
    }
}
