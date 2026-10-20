//! Member sign-in settings for the public origin (`[public]`). The portal
//! opens with the live admin switch `self_service.public_portal` (D7-B); there
//! is no open key. Secrets are files in the secrets directory, as for admin.

use std::{collections::BTreeMap, time::Duration};

use super::{DiscordOAuthSettings, Error, HttpConfig, parse_bounded_u64, refuse_plain};
use crate::runtime::secrets;

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
        refuse_plain(
            values,
            "KANADE_PUBLIC_DISCORD_CLIENT_SECRET",
            secrets::PUBLIC_DISCORD_CLIENT_SECRET,
        )?;
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
    DiscordOAuthSettings::from_id(
        values,
        "KANADE_PUBLIC_DISCORD_CLIENT_ID",
        secrets::PUBLIC_DISCORD_CLIENT_SECRET,
        || {
            let public_host = http.public_host.as_deref().ok_or_else(|| {
                Error::Configuration("KANADE_PUBLIC_HOST is required for member sign-in".into())
            })?;
            // https only: the public origin is never a loopback dev host.
            Ok(format!("https://{public_host}{CALLBACK_PATH}"))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::super::RuntimeConfig;

    fn config(pairs: &[(&str, &str)]) -> Result<RuntimeConfig, String> {
        let mut values = std::collections::BTreeMap::from([
            ("KANADE_TIMEZONE".to_owned(), "Asia/Kuala_Lumpur".to_owned()),
            ("KANADE_SECRETS_DIR".to_owned(), "/run/secrets".to_owned()),
            // Compose's fixed public listener; the host turns it on.
            ("KANADE_PUBLIC_BIND".to_owned(), "127.0.0.1:8081".to_owned()),
            ("KANADE_CLOUDFLARED_PEER".to_owned(), "127.0.0.1".to_owned()),
        ]);
        for (key, value) in pairs {
            values.insert((*key).into(), (*value).into());
        }
        RuntimeConfig::from_mapping(&values).map_err(|error| error.to_string())
    }

    const DISCORD: [(&str, &str); 2] = [
        ("KANADE_PUBLIC_HOST", "kanade-pub.example"),
        ("KANADE_PUBLIC_DISCORD_CLIENT_ID", "1234567890"),
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
    fn the_client_id_turns_member_sign_in_on_with_the_computed_https_callback() {
        let discord = config(&DISCORD).unwrap().public_auth.discord.unwrap();
        assert_eq!(discord.client_id, "1234567890");
        assert_eq!(
            discord.redirect_uri,
            "https://kanade-pub.example/api/public/auth/discord/callback"
        );
        assert_eq!(
            discord.client_secret.path(),
            std::path::Path::new("/run/secrets/public_discord_client_secret")
        );
        assert!(
            config(&DISCORD[1..])
                .unwrap_err()
                .contains("KANADE_PUBLIC_HOST is required")
        );
        // No http exception on the public origin.
        let local = config(&[("KANADE_PUBLIC_HOST", "localhost:4394"), DISCORD[1]]).unwrap();
        assert_eq!(
            local.public_auth.discord.unwrap().redirect_uri,
            "https://localhost:4394/api/public/auth/discord/callback"
        );
        let bad_id = [
            DISCORD[0],
            ("KANADE_PUBLIC_DISCORD_CLIENT_ID", "not-a-snowflake"),
        ];
        assert!(config(&bad_id).unwrap_err().contains("application id"));
    }

    #[test]
    fn a_plain_public_secret_is_refused() {
        assert_eq!(
            config(&[("KANADE_PUBLIC_DISCORD_CLIENT_SECRET", "hunter2")]).unwrap_err(),
            "KANADE_PUBLIC_DISCORD_CLIENT_SECRET is not read; put it in the secrets directory as `public_discord_client_secret`"
        );
    }
}
