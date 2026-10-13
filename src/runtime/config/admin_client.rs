//! `kanade portal …` and `kanade sessions …`: where the running bot's admin
//! listener is, the `Host`/`Origin` it accepts and the break-glass token
//! file to sign in with. Nothing else is read.

use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, str::FromStr};

use super::{
    DEFAULT_BIND, Error, allow_private_bind, host, is_private, listener_bind, non_empty,
    refuse_plain,
};

pub const TOKEN_FILE_KEY: &str = "KANADE_ADMIN_TOKEN_FILE";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminClientConfig {
    /// `--url`, else `KANADE_ADMIN_BIND`: loopback, or private with
    /// `KANADE_ALLOW_PRIVATE_BIND=1` (the container's own listener).
    pub target: SocketAddr,
    /// The `Host` sent: `KANADE_ADMIN_HOST`, else `localhost:<port>` (an
    /// unconfigured admin listener accepts loopback names only).
    pub authority: String,
    /// The listener's own origin: `https://` + the admin host, or `http://`
    /// + the loopback name.
    pub origin: String,
    pub token_file: PathBuf,
}

impl AdminClientConfig {
    pub fn from_mapping(
        values: &BTreeMap<String, String>,
        url: Option<&str>,
    ) -> Result<Self, Error> {
        // Same rule as serve.
        refuse_plain(values, "KANADE_ADMIN_TOKEN", TOKEN_FILE_KEY)?;
        let private = allow_private_bind(values)?;
        let target = match url {
            Some(url) => parse_url(url, private)?,
            None => listener_bind(
                "KANADE_ADMIN_BIND",
                non_empty(values, "KANADE_ADMIN_BIND").unwrap_or(DEFAULT_BIND),
                private,
            )?,
        };
        let token_file = non_empty(values, TOKEN_FILE_KEY)
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::Configuration(format!(
                    "{TOKEN_FILE_KEY} is required: the break-glass token signs this command in"
                ))
            })?;
        let (authority, origin) = match host(values, "KANADE_ADMIN_HOST")? {
            Some(host) => {
                let origin = format!("https://{host}");
                (host, origin)
            }
            None => {
                let authority = format!("localhost:{}", target.port());
                let origin = format!("http://{authority}");
                (authority, origin)
            }
        };
        Ok(Self {
            target,
            authority,
            origin,
            token_file,
        })
    }
}

/// `http://HOST:PORT` (a trailing `/` allowed), with the bind rule.
fn parse_url(value: &str, private: bool) -> Result<SocketAddr, Error> {
    let address = value
        .strip_prefix("http://")
        .map(|rest| rest.strip_suffix('/').unwrap_or(rest))
        .and_then(|address| SocketAddr::from_str(address).ok())
        .ok_or_else(|| Error::Configuration("--url must be http://LOOPBACK:PORT".into()))?;
    if address.ip().is_loopback() || (private && is_private(address.ip())) {
        Ok(address)
    } else {
        Err(Error::Configuration(
            "--url must name the admin listener's loopback address".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)], url: Option<&str>) -> Result<AdminClientConfig, String> {
        let values = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        AdminClientConfig::from_mapping(&values, url).map_err(|error| error.to_string())
    }

    const TOKEN: (&str, &str) = (TOKEN_FILE_KEY, "/run/secrets/kanade_admin_token");

    #[test]
    fn defaults_to_the_admin_bind_with_a_loopback_origin() {
        let default = config(&[TOKEN], None).unwrap();
        assert_eq!(default.target, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(default.authority, "localhost:8080");
        assert_eq!(default.origin, "http://localhost:8080");
        assert_eq!(
            default.token_file,
            PathBuf::from("/run/secrets/kanade_admin_token")
        );
        let overridden = config(
            &[TOKEN, ("KANADE_ADMIN_BIND", "127.0.0.1:9000")],
            Some("http://[::1]:4393/"),
        )
        .unwrap();
        assert_eq!(overridden.target, "[::1]:4393".parse().unwrap());
        assert_eq!(overridden.origin, "http://localhost:4393");
    }

    #[test]
    fn the_admin_host_names_host_and_origin() {
        let config = config(
            &[
                TOKEN,
                ("KANADE_ALLOW_PRIVATE_BIND", "1"),
                ("KANADE_ADMIN_BIND", "192.168.97.10:8080"),
                ("KANADE_ADMIN_HOST", "Kanade.Example"),
            ],
            None,
        )
        .unwrap();
        assert_eq!(config.target, "192.168.97.10:8080".parse().unwrap());
        assert_eq!(config.authority, "kanade.example");
        assert_eq!(config.origin, "https://kanade.example");
    }

    #[test]
    fn refuses_a_plain_token_a_missing_file_and_non_loopback_targets() {
        assert_eq!(
            config(&[TOKEN, ("KANADE_ADMIN_TOKEN", "hunter2")], None).unwrap_err(),
            "KANADE_ADMIN_TOKEN is not read; use KANADE_ADMIN_TOKEN_FILE"
        );
        assert!(config(&[], None).unwrap_err().starts_with(TOKEN_FILE_KEY));
        for url in [
            "http://10.0.0.1:8080",
            "https://127.0.0.1:8080",
            "http://localhost:8080",
            "http://127.0.0.1:8080/healthz",
        ] {
            assert!(config(&[TOKEN], Some(url)).is_err(), "{url}");
        }
        assert_eq!(
            config(
                &[TOKEN, ("KANADE_ALLOW_PRIVATE_BIND", "1")],
                Some("http://10.0.0.1:8080")
            )
            .unwrap()
            .target,
            "10.0.0.1:8080".parse().unwrap()
        );
        assert!(config(&[TOKEN, ("KANADE_ADMIN_BIND", "0.0.0.0:8080")], None).is_err());
    }
}
