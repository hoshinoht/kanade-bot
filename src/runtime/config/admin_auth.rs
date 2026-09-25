//! Admin sign-in settings. Secrets are file paths only; plain-value
//! variables are refused so a secret never sits in the process environment.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use super::{Error, HttpConfig, non_empty, parse_bounded_u64};

/// The only callback path the admin origin serves.
const CALLBACK_PATH: &str = "/api/admin/auth/discord/callback";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscordOAuthSettings {
    pub client_id: String,
    pub client_secret_file: PathBuf,
    /// Exact redirect URI registered with Discord.
    pub redirect_uri: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminAuthSettings {
    pub discord: Option<DiscordOAuthSettings>,
    pub token_file: Option<PathBuf>,
    /// Lowercase Tailscale logins allowed to sign in through the edge.
    pub tailscale_logins: Vec<String>,
    pub session_idle: Duration,
    pub session_absolute: Duration,
}

impl Default for AdminAuthSettings {
    fn default() -> Self {
        Self {
            discord: None,
            token_file: None,
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
        for plain in ["KANADE_ADMIN_TOKEN", "KANADE_ADMIN_DISCORD_CLIENT_SECRET"] {
            if non_empty(values, plain).is_some() {
                return Err(Error::Configuration(format!(
                    "{plain} is not read; use {plain}_FILE"
                )));
            }
        }
        let tailscale_logins = tailscale_logins(values)?;
        if !tailscale_logins.is_empty() && http.trusted_proxy.is_none() {
            return Err(Error::Configuration(
                "KANADE_ADMIN_TAILSCALE_LOGINS requires KANADE_TRUSTED_PROXY".into(),
            ));
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
        Ok(Self {
            discord: discord(values, http)?,
            token_file: non_empty(values, "KANADE_ADMIN_TOKEN_FILE").map(PathBuf::from),
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

fn discord(
    values: &BTreeMap<String, String>,
    http: &HttpConfig,
) -> Result<Option<DiscordOAuthSettings>, Error> {
    let parts = [
        non_empty(values, "KANADE_ADMIN_DISCORD_CLIENT_ID"),
        non_empty(values, "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE"),
        non_empty(values, "KANADE_ADMIN_DISCORD_REDIRECT_URI"),
    ];
    let [Some(client_id), Some(secret_file), Some(redirect_uri)] = parts else {
        if parts.iter().any(Option::is_some) {
            return Err(Error::Configuration(
                "Discord sign-in needs KANADE_ADMIN_DISCORD_CLIENT_ID, \
                 KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE and KANADE_ADMIN_DISCORD_REDIRECT_URI"
                    .into(),
            ));
        }
        return Ok(None);
    };
    if client_id.is_empty()
        || client_id.len() > 20
        || !client_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(Error::Configuration(
            "KANADE_ADMIN_DISCORD_CLIENT_ID must be a Discord application id".into(),
        ));
    }
    let admin_host = http.admin_host.as_deref().ok_or_else(|| {
        Error::Configuration("KANADE_ADMIN_HOST is required for Discord sign-in".into())
    })?;
    if !redirect_matches(redirect_uri, admin_host) {
        return Err(Error::Configuration(format!(
            "KANADE_ADMIN_DISCORD_REDIRECT_URI must be https://KANADE_ADMIN_HOST{CALLBACK_PATH}"
        )));
    }
    Ok(Some(DiscordOAuthSettings {
        client_id: client_id.to_owned(),
        client_secret_file: PathBuf::from(secret_file),
        redirect_uri: redirect_uri.to_owned(),
    }))
}

/// Exactly `https://{admin_host}{CALLBACK_PATH}`; `http:` only for loopback development hosts.
fn redirect_matches(uri: &str, admin_host: &str) -> bool {
    let loopback = ["localhost", "127.0.0.1", "[::1]"].iter().any(|name| {
        admin_host
            .strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
    });
    uri == format!("https://{admin_host}{CALLBACK_PATH}")
        || (loopback && uri == format!("http://{admin_host}{CALLBACK_PATH}"))
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
        ("KANADE_ADMIN_HOST", "kanade.example"),
        ("KANADE_ADMIN_DISCORD_CLIENT_ID", "1234567890"),
        (
            "KANADE_ADMIN_DISCORD_CLIENT_SECRET_FILE",
            "/run/secrets/discord",
        ),
        (
            "KANADE_ADMIN_DISCORD_REDIRECT_URI",
            "https://kanade.example/api/admin/auth/discord/callback",
        ),
    ];

    #[test]
    fn defaults_have_no_sign_in_and_bounded_sessions() {
        let auth = config(&[]).unwrap().admin_auth;
        assert_eq!(auth, super::AdminAuthSettings::default());
        assert_eq!(auth.session_idle.as_secs(), 3600);
    }

    #[test]
    fn discord_needs_all_parts_and_the_exact_admin_callback() {
        let auth = config(&DISCORD).unwrap().admin_auth;
        assert_eq!(auth.discord.unwrap().client_id, "1234567890");
        assert!(config(&DISCORD[..3]).unwrap_err().contains("needs"));
        for bad in [
            "https://evil.example/api/admin/auth/discord/callback",
            "http://kanade.example/api/admin/auth/discord/callback",
            "https://kanade.example/api/admin/auth/discord/callback?x=1",
            "https://kanade.example/other",
        ] {
            let mut pairs = DISCORD.to_vec();
            pairs[3].1 = bad;
            let error = config(&pairs).unwrap_err();
            assert!(error.contains("must be https://"), "{bad}");
            assert!(!error.contains("evil"));
        }
        let mut local = DISCORD.to_vec();
        local[0].1 = "localhost:4393";
        local[3].1 = "http://localhost:4393/api/admin/auth/discord/callback";
        assert!(config(&local).is_ok());
    }

    #[test]
    fn plain_secrets_and_orphan_tailscale_logins_are_refused() {
        assert_eq!(
            config(&[("KANADE_ADMIN_TOKEN", "hunter2")]).unwrap_err(),
            "KANADE_ADMIN_TOKEN is not read; use KANADE_ADMIN_TOKEN_FILE"
        );
        assert!(
            config(&[("KANADE_ADMIN_TAILSCALE_LOGINS", "a@example.com")])
                .unwrap_err()
                .contains("requires KANADE_TRUSTED_PROXY")
        );
        let auth = config(&[
            ("KANADE_TRUSTED_PROXY", "127.0.0.1"),
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
                ("KANADE_TRUSTED_PROXY", "127.0.0.1"),
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
