//! Admin sign-in settings. Secrets are files in the secrets directory only;
//! plain-value variables are refused so a secret never sits in the process
//! environment.

use std::{collections::BTreeMap, path::Path, time::Duration};

use super::{Error, HttpConfig, non_empty, parse_bounded_u64, refuse_plain};
use crate::runtime::secrets::{self, DEFAULT_DIR, SecretFile};

/// The only callback path the admin origin serves.
const CALLBACK_PATH: &str = "/api/admin/auth/discord/callback";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordOAuthSettings {
    pub client_id: String,
    /// Required while the client id is set; read at startup.
    pub client_secret: SecretFile,
    /// The origin's own callback, registered with Discord.
    pub redirect_uri: String,
}

impl DiscordOAuthSettings {
    /// `None` without `id_key`; else its application id, the secret file
    /// `secret` and `redirect_uri`.
    pub(super) fn from_id(
        values: &BTreeMap<String, String>,
        id_key: &str,
        secret: &'static str,
        redirect_uri: impl FnOnce() -> Result<String, Error>,
    ) -> Result<Option<Self>, Error> {
        let Some(client_id) = non_empty(values, id_key) else {
            return Ok(None);
        };
        if client_id.len() > 20 || !client_id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(Error::Configuration(format!(
                "{id_key} must be a Discord application id"
            )));
        }
        Ok(Some(Self {
            client_id: client_id.to_owned(),
            client_secret: SecretFile::named(values, secret),
            redirect_uri: redirect_uri()?,
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminAuthSettings {
    pub discord: Option<DiscordOAuthSettings>,
    /// Break-glass sign-in while `admin_token` is present.
    pub token: SecretFile,
    /// Lowercase Tailscale logins allowed to sign in through the edge.
    pub tailscale_logins: Vec<String>,
    pub session_idle: Duration,
    pub session_absolute: Duration,
}

impl Default for AdminAuthSettings {
    fn default() -> Self {
        Self {
            discord: None,
            token: SecretFile::new(Path::new(DEFAULT_DIR), secrets::ADMIN_TOKEN),
            tailscale_logins: Vec::new(),
            session_idle: Duration::from_secs(60 * 60),
            session_absolute: Duration::from_secs(12 * 60 * 60),
        }
    }
}

impl AdminAuthSettings {
    pub(super) fn from_mapping(
        values: &BTreeMap<String, String>,
        http: &HttpConfig,
    ) -> Result<Self, Error> {
        refuse_plain(values, "KANADE_ADMIN_TOKEN", secrets::ADMIN_TOKEN)?;
        refuse_plain(
            values,
            "KANADE_ADMIN_DISCORD_CLIENT_SECRET",
            secrets::DISCORD_CLIENT_SECRET,
        )?;
        let tailscale_logins = tailscale_logins(values)?;
        if !tailscale_logins.is_empty() {
            match http.trusted_proxy {
                None => {
                    return Err(Error::Configuration(
                        "KANADE_ADMIN_TAILSCALE_LOGINS requires KANADE_TRUSTED_PROXY".into(),
                    ));
                }
                // Any local process shares a loopback peer address with the edge.
                Some(proxy) if proxy.is_loopback() => {
                    return Err(Error::Configuration(
                        "KANADE_ADMIN_TAILSCALE_LOGINS needs a non-loopback KANADE_TRUSTED_PROXY"
                            .into(),
                    ));
                }
                // The edge secret it also needs is checked at startup.
                Some(_) => {}
            }
        }
        let idle_minutes =
            parse_bounded_u64(values, "KANADE_ADMIN_SESSION_IDLE_MINUTES", 60, 5, 720)?;
        let absolute_hours =
            parse_bounded_u64(values, "KANADE_ADMIN_SESSION_ABSOLUTE_HOURS", 12, 1, 168)?;
        if idle_minutes > absolute_hours * 60 {
            return Err(Error::Configuration(
                "KANADE_ADMIN_SESSION_IDLE_MINUTES must not exceed the absolute lifetime".into(),
            ));
        }
        let discord = DiscordOAuthSettings::from_id(
            values,
            "KANADE_ADMIN_DISCORD_CLIENT_ID",
            secrets::DISCORD_CLIENT_SECRET,
            || {
                let host = http.admin_host.as_deref().ok_or_else(|| {
                    Error::Configuration("KANADE_ADMIN_HOST is required for Discord sign-in".into())
                })?;
                Ok(redirect_uri(host))
            },
        )?;
        Ok(Self {
            discord,
            token: SecretFile::named(values, secrets::ADMIN_TOKEN),
            tailscale_logins,
            session_idle: Duration::from_secs(idle_minutes * 60),
            session_absolute: Duration::from_secs(absolute_hours * 60 * 60),
        })
    }
}

fn tailscale_logins(values: &BTreeMap<String, String>) -> Result<Vec<String>, Error> {
    let Some(list) = non_empty(values, "KANADE_ADMIN_TAILSCALE_LOGINS") else {
        return Ok(Vec::new());
    };
    list.split(',')
        .map(str::trim)
        .filter(|login| !login.is_empty())
        .map(|login| {
            let ok = login.len() <= 320
                && login.contains('@')
                && login.bytes().all(|byte| byte.is_ascii_graphic());
            ok.then(|| login.to_ascii_lowercase()).ok_or_else(|| {
                Error::Configuration(
                    "KANADE_ADMIN_TAILSCALE_LOGINS must be comma-separated logins".into(),
                )
            })
        })
        .collect()
}

/// `https://{admin_host}{CALLBACK_PATH}`; `http:` for loopback development
/// hosts.
fn redirect_uri(admin_host: &str) -> String {
    let loopback = ["localhost", "127.0.0.1", "[::1]"].iter().any(|name| {
        admin_host
            .strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
    });
    let scheme = if loopback { "http" } else { "https" };
    format!("{scheme}://{admin_host}{CALLBACK_PATH}")
}

#[cfg(test)]
mod tests {
    use super::super::RuntimeConfig;

    fn config(pairs: &[(&str, &str)]) -> Result<RuntimeConfig, String> {
        let mut values = std::collections::BTreeMap::from([
            ("KANADE_TIMEZONE".to_owned(), "Asia/Kuala_Lumpur".to_owned()),
            ("KANADE_SECRETS_DIR".to_owned(), "/run/secrets".to_owned()),
        ]);
        for (key, value) in pairs {
            values.insert((*key).into(), (*value).into());
        }
        RuntimeConfig::from_mapping(&values).map_err(|error| error.to_string())
    }

    const DISCORD: [(&str, &str); 2] = [
        ("KANADE_ADMIN_HOST", "kanade.example"),
        ("KANADE_ADMIN_DISCORD_CLIENT_ID", "1234567890"),
    ];

    #[test]
    fn defaults_have_no_sign_in_and_bounded_sessions() {
        let auth = config(&[]).unwrap().admin_auth;
        assert_eq!(auth, super::AdminAuthSettings::default());
        assert_eq!(auth.session_idle.as_secs(), 3600);
        assert_eq!(
            auth.token.path(),
            std::path::Path::new("/run/secrets/admin_token")
        );
    }

    #[test]
    fn the_client_id_turns_discord_on_with_the_computed_admin_callback() {
        let discord = config(&DISCORD).unwrap().admin_auth.discord.unwrap();
        assert_eq!(discord.client_id, "1234567890");
        assert_eq!(
            discord.redirect_uri,
            "https://kanade.example/api/admin/auth/discord/callback"
        );
        assert_eq!(
            discord.client_secret.path(),
            std::path::Path::new("/run/secrets/discord_client_secret")
        );
        assert_eq!(
            config(&DISCORD[1..]).unwrap_err(),
            "KANADE_ADMIN_HOST is required for Discord sign-in"
        );
        assert!(
            config(&[DISCORD[0], ("KANADE_ADMIN_DISCORD_CLIENT_ID", "not-an-id")])
                .unwrap_err()
                .contains("application id")
        );
        for (host, uri) in [
            (
                "localhost:4393",
                "http://localhost:4393/api/admin/auth/discord/callback",
            ),
            (
                "127.0.0.1:4393",
                "http://127.0.0.1:4393/api/admin/auth/discord/callback",
            ),
            (
                "localhost.example",
                "https://localhost.example/api/admin/auth/discord/callback",
            ),
        ] {
            let local = config(&[("KANADE_ADMIN_HOST", host), DISCORD[1]]).unwrap();
            assert_eq!(local.admin_auth.discord.unwrap().redirect_uri, uri);
        }
    }

    #[test]
    fn plain_secrets_and_orphan_tailscale_logins_are_refused() {
        assert_eq!(
            config(&[("KANADE_ADMIN_TOKEN", "hunter2")]).unwrap_err(),
            "KANADE_ADMIN_TOKEN is not read; put it in the secrets directory as `admin_token`"
        );
        assert_eq!(
            config(&[("KANADE_ADMIN_DISCORD_CLIENT_SECRET", "hunter2")]).unwrap_err(),
            "KANADE_ADMIN_DISCORD_CLIENT_SECRET is not read; put it in the secrets directory as `discord_client_secret`"
        );
        assert!(
            config(&[("KANADE_ADMIN_TAILSCALE_LOGINS", "a@example.com")])
                .unwrap_err()
                .contains("requires KANADE_TRUSTED_PROXY")
        );
        assert_eq!(
            config(&[
                ("KANADE_TRUSTED_PROXY", "127.0.0.1"),
                ("KANADE_ADMIN_TAILSCALE_LOGINS", "a@example.com"),
            ])
            .unwrap_err(),
            "KANADE_ADMIN_TAILSCALE_LOGINS needs a non-loopback KANADE_TRUSTED_PROXY"
        );
        let auth = config(&[
            ("KANADE_TRUSTED_PROXY", "172.18.0.2"),
            (
                "KANADE_ADMIN_TAILSCALE_LOGINS",
                " A@Example.com , b@github ",
            ),
        ])
        .unwrap()
        .admin_auth;
        assert_eq!(auth.tailscale_logins, ["a@example.com", "b@github"]);
        assert!(
            config(&[
                ("KANADE_TRUSTED_PROXY", "172.18.0.2"),
                ("KANADE_ADMIN_TAILSCALE_LOGINS", "not a login"),
            ])
            .is_err()
        );
        assert!(
            config(&[
                ("KANADE_ADMIN_SESSION_IDLE_MINUTES", "120"),
                ("KANADE_ADMIN_SESSION_ABSOLUTE_HOURS", "1"),
            ])
            .is_err()
        );
    }
}
