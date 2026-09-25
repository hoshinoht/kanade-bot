use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

use chrono_tz::Tz;

use super::error::Error;

mod admin_auth;

pub use admin_auth::{AdminAuthSettings, DiscordOAuthSettings};

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_SHUTDOWN_SECONDS: u64 = 10;
const DEFAULT_HEALTHCHECK_TIMEOUT_SECONDS: u64 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub admin_bind: SocketAddr,
    /// The public listener exists only when this is set.
    pub public_bind: Option<SocketAddr>,
    pub http: HttpConfig,
    pub admin_auth: AdminAuthSettings,
    pub timezone: Tz,
    pub shutdown_timeout: Duration,
}

/// Per-origin HTTP policy shared by both listeners.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HttpConfig {
    /// Lowercase `host[:port]`; unset admin accepts loopback names only.
    pub admin_host: Option<String>,
    /// Required whenever the public listener is configured.
    pub public_host: Option<String>,
    /// Edge peer allowed to supply forwarding and Tailscale identity headers to admin.
    pub trusted_proxy: Option<IpAddr>,
    /// cloudflared peer allowed to supply `CF-Connecting-IP` to public.
    pub cloudflared_peer: Option<IpAddr>,
    /// Web workspace root holding `apps/{admin,public}/dist`.
    pub web_dir: Option<PathBuf>,
    pub boss_dir: Option<PathBuf>,
    pub identity_dir: Option<PathBuf>,
    /// Secret the edge proves itself with (`X-Kanade-Edge-Auth`); when set,
    /// admin trusts the proxy's headers only with it.
    pub edge_secret_file: Option<PathBuf>,
}

impl RuntimeConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        // Refused rather than aliased so a stale setting cannot silently fall back to the default bind.
        if non_empty(values, "KANADE_BIND").is_some() {
            return Err(Error::Configuration(
                "KANADE_BIND was renamed to KANADE_ADMIN_BIND".into(),
            ));
        }
        let private = allow_private_bind(values)?;
        let admin_bind = listener_bind(
            "KANADE_ADMIN_BIND",
            non_empty(values, "KANADE_ADMIN_BIND").unwrap_or(DEFAULT_BIND),
            private,
        )?;
        let public_bind = non_empty(values, "KANADE_PUBLIC_BIND")
            .map(|value| listener_bind("KANADE_PUBLIC_BIND", value, private))
            .transpose()?;
        if public_bind.is_some_and(|public| public == admin_bind && public.port() != 0) {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_BIND must differ from KANADE_ADMIN_BIND".into(),
            ));
        }
        let http = HttpConfig {
            admin_host: host(values, "KANADE_ADMIN_HOST")?,
            public_host: host(values, "KANADE_PUBLIC_HOST")?,
            trusted_proxy: peer(values, "KANADE_TRUSTED_PROXY")?,
            cloudflared_peer: peer(values, "KANADE_CLOUDFLARED_PEER")?,
            web_dir: non_empty(values, "KANADE_WEB_DIR").map(PathBuf::from),
            boss_dir: non_empty(values, "KANADE_BOSS_DIR").map(PathBuf::from),
            identity_dir: non_empty(values, "KANADE_IDENTITY_DIR").map(PathBuf::from),
            edge_secret_file: non_empty(values, "KANADE_EDGE_SECRET_FILE").map(PathBuf::from),
        };
        if http.edge_secret_file.is_some() && http.trusted_proxy.is_none() {
            return Err(Error::Configuration(
                "KANADE_EDGE_SECRET_FILE requires KANADE_TRUSTED_PROXY".into(),
            ));
        }
        if public_bind.is_some() && http.public_host.is_none() {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_HOST is required when KANADE_PUBLIC_BIND is set".into(),
            ));
        }
        if http.public_host.is_some() && http.public_host == http.admin_host {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_HOST must differ from KANADE_ADMIN_HOST".into(),
            ));
        }
        let admin_auth = AdminAuthSettings::from_mapping(values, &http)?;
        let timezone = values
            .get("KANADE_TIMEZONE")
            .ok_or_else(|| Error::Configuration("KANADE_TIMEZONE is required".into()))?
            .parse::<Tz>()
            .map_err(|_| {
                Error::Configuration("KANADE_TIMEZONE must be a valid IANA timezone".into())
            })?;

        Ok(Self {
            admin_bind,
            public_bind,
            http,
            admin_auth,
            timezone,
            shutdown_timeout: Duration::from_secs(parse_bounded_u64(
                values,
                "KANADE_SHUTDOWN_TIMEOUT_SECONDS",
                DEFAULT_SHUTDOWN_SECONDS,
                1,
                120,
            )?),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthcheckConfig {
    pub target: SocketAddr,
    pub timeout: Duration,
}

impl HealthcheckConfig {
    pub fn from_mapping(
        values: &BTreeMap<String, String>,
        url: Option<&str>,
    ) -> Result<Self, Error> {
        let target = url
            .map(str::to_owned)
            .or_else(|| values.get("KANADE_HEALTHCHECK_URL").cloned())
            .ok_or_else(|| {
                Error::Configuration(
                    "KANADE_HEALTHCHECK_URL or healthcheck --url is required".into(),
                )
            })?;
        Ok(Self {
            target: parse_health_url(&target, allow_private_bind(values)?)?,
            timeout: Duration::from_secs(parse_bounded_u64(
                values,
                "KANADE_HEALTHCHECK_TIMEOUT_SECONDS",
                DEFAULT_HEALTHCHECK_TIMEOUT_SECONDS,
                1,
                30,
            )?),
        })
    }
}

/// Empty values count as unset, as Compose passes them for blank variables.
fn non_empty<'a>(values: &'a BTreeMap<String, String>, key: &str) -> Option<&'a str> {
    values
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

/// `KANADE_ALLOW_PRIVATE_BIND=1` lets listeners bind a private address on an
/// internal container network (the edge's); loopback stays the default.
fn allow_private_bind(values: &BTreeMap<String, String>) -> Result<bool, Error> {
    match non_empty(values, "KANADE_ALLOW_PRIVATE_BIND") {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(Error::Configuration(
            "KANADE_ALLOW_PRIVATE_BIND must be 0 or 1".into(),
        )),
    }
}

/// RFC 1918 and IPv4 link-local; IPv6 unique-local (fc00::/7) and link-local (fe80::/10).
pub fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

fn listener_bind(key: &str, value: &str, private: bool) -> Result<SocketAddr, Error> {
    let bind = value
        .parse::<SocketAddr>()
        .map_err(|_| Error::Configuration(format!("{key} must be an IP address and port")))?;
    let ip = bind.ip();
    if ip.is_loopback() || (private && is_private(ip)) {
        return Ok(bind);
    }
    Err(Error::Configuration(if private {
        format!("{key} must be a loopback or private address")
    } else {
        format!("{key} must be a loopback address")
    }))
}

fn peer(values: &BTreeMap<String, String>, key: &str) -> Result<Option<IpAddr>, Error> {
    non_empty(values, key)
        .map(|value| {
            value
                .parse::<IpAddr>()
                .map_err(|_| Error::Configuration(format!("{key} must be an IP address")))
        })
        .transpose()
}

fn host(values: &BTreeMap<String, String>, key: &str) -> Result<Option<String>, Error> {
    non_empty(values, key)
        .map(|value| {
            let value = value.to_ascii_lowercase();
            if is_authority(&value) {
                Ok(value)
            } else {
                Err(Error::Configuration(format!(
                    "{key} must be a host name with an optional port"
                )))
            }
        })
        .transpose()
}

/// `name[:port]` or `[v6][:port]`; no scheme, path, userinfo or wildcard.
fn is_authority(value: &str) -> bool {
    let (name, port) = match value.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((v6, tail)) => {
                if v6.parse::<std::net::Ipv6Addr>().is_err() {
                    return false;
                }
                match tail {
                    "" => return true,
                    _ => match tail.strip_prefix(':') {
                        Some(port) => ("v6", Some(port)),
                        None => return false,
                    },
                }
            }
            None => return false,
        },
        None => match value.split_once(':') {
            Some((name, port)) => (name, Some(port)),
            None => (value, None),
        },
    };
    let name_ok = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        });
    name_ok
        && port.is_none_or(|port| {
            port.bytes().all(|byte| byte.is_ascii_digit())
                && port.parse::<u16>().is_ok_and(|port| port != 0)
        })
}

fn parse_bounded_u64(
    values: &BTreeMap<String, String>,
    key: &str,
    default: u64,
    minimum: u64,
    maximum: u64,
) -> Result<u64, Error> {
    let value = match values.get(key) {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| Error::Configuration(format!("{key} must be an integer")))?,
        None => default,
    };
    if !(minimum..=maximum).contains(&value) {
        return Err(Error::Configuration(format!(
            "{key} must be between {minimum} and {maximum}"
        )));
    }
    Ok(value)
}

/// Loopback, or with the private-bind opt-in the listener's own private address.
fn parse_health_url(value: &str, private: bool) -> Result<SocketAddr, Error> {
    let address = value
        .strip_prefix("http://")
        .and_then(|remainder| remainder.strip_suffix("/healthz"))
        .and_then(|address| SocketAddr::from_str(address).ok())
        .ok_or_else(|| {
            Error::Configuration("healthcheck URL must be http://LOOPBACK:PORT/healthz".into())
        })?;
    if !(address.ip().is_loopback() || (private && is_private(address.ip()))) {
        return Err(Error::Configuration(
            "healthcheck target must be loopback".into(),
        ));
    }
    Ok(address)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values() -> BTreeMap<String, String> {
        BTreeMap::from([("KANADE_TIMEZONE".into(), "Asia/Kuala_Lumpur".into())])
    }

    #[test]
    fn configuration_is_loaded_without_process_environment() {
        let config = RuntimeConfig::from_mapping(&values()).unwrap();
        assert_eq!(config.admin_bind.to_string(), DEFAULT_BIND);
        assert_eq!(config.public_bind, None);
        assert_eq!(config.http, HttpConfig::default());
        assert_eq!(config.timezone, chrono_tz::Asia::Kuala_Lumpur);
    }

    fn error_for(pairs: &[(&str, &str)]) -> String {
        let mut input = values();
        for (key, value) in pairs {
            input.insert((*key).into(), (*value).into());
        }
        RuntimeConfig::from_mapping(&input).unwrap_err().to_string()
    }

    #[test]
    fn non_loopback_is_rejected_even_with_a_legacy_allowance() {
        assert_eq!(
            error_for(&[
                ("KANADE_ADMIN_BIND", "0.0.0.0:8080"),
                ("KANADE_ALLOW_NON_LOOPBACK", "true"),
            ]),
            "KANADE_ADMIN_BIND must be a loopback address"
        );
        assert_eq!(
            error_for(&[
                ("KANADE_PUBLIC_BIND", "0.0.0.0:8081"),
                ("KANADE_PUBLIC_HOST", "kanade-pub.example"),
            ]),
            "KANADE_PUBLIC_BIND must be a loopback address"
        );
    }

    #[test]
    fn ipv6_non_loopback_is_rejected() {
        assert_eq!(
            error_for(&[("KANADE_ADMIN_BIND", "[2001:db8::1]:8080")]),
            "KANADE_ADMIN_BIND must be a loopback address"
        );
    }

    #[test]
    fn legacy_bind_is_refused_without_echoing_it() {
        let error = error_for(&[("KANADE_BIND", "127.0.0.9:8080")]);
        assert_eq!(error, "KANADE_BIND was renamed to KANADE_ADMIN_BIND");
    }

    #[test]
    fn public_listener_is_optional_but_needs_its_own_host() {
        assert_eq!(
            error_for(&[("KANADE_PUBLIC_BIND", "127.0.0.1:8081")]),
            "KANADE_PUBLIC_HOST is required when KANADE_PUBLIC_BIND is set"
        );
        assert_eq!(
            error_for(&[
                ("KANADE_PUBLIC_BIND", "127.0.0.1:8080"),
                ("KANADE_PUBLIC_HOST", "pub.example"),
            ]),
            "KANADE_PUBLIC_BIND must differ from KANADE_ADMIN_BIND"
        );
        assert_eq!(
            error_for(&[
                ("KANADE_ADMIN_HOST", "Same.Example"),
                ("KANADE_PUBLIC_HOST", "same.example"),
            ]),
            "KANADE_PUBLIC_HOST must differ from KANADE_ADMIN_HOST"
        );

        let mut input = values();
        for (key, value) in [
            ("KANADE_ADMIN_BIND", "127.0.0.1:8080"),
            ("KANADE_PUBLIC_BIND", "[::1]:8081"),
            ("KANADE_ADMIN_HOST", "Kanade.Example"),
            ("KANADE_PUBLIC_HOST", "kanade-pub.example:8443"),
            ("KANADE_TRUSTED_PROXY", "127.0.0.2"),
            ("KANADE_CLOUDFLARED_PEER", "::1"),
            ("KANADE_WEB_DIR", "web"),
            ("KANADE_BOSS_DIR", ""),
        ] {
            input.insert(key.into(), value.into());
        }
        let config = RuntimeConfig::from_mapping(&input).unwrap();
        assert_eq!(config.public_bind.unwrap().to_string(), "[::1]:8081");
        assert_eq!(config.http.admin_host.as_deref(), Some("kanade.example"));
        assert_eq!(
            config.http.public_host.as_deref(),
            Some("kanade-pub.example:8443")
        );
        assert_eq!(config.http.trusted_proxy, Some([127, 0, 0, 2].into()));
        assert_eq!(config.http.web_dir, Some(PathBuf::from("web")));
        assert_eq!(config.http.boss_dir, None);
    }

    #[test]
    fn hosts_and_peers_are_validated_without_echoing_values() {
        for bad in [
            "https://kanade.example",
            "kanade.example/path",
            "user@kanade.example",
            "*.example",
            "kanade.example:0",
            "kanade.example:+80",
            "kanade..example",
            "[not-v6]:80",
            "-kanade.example",
        ] {
            let error = error_for(&[("KANADE_ADMIN_HOST", bad)]);
            assert_eq!(
                error, "KANADE_ADMIN_HOST must be a host name with an optional port",
                "{bad}"
            );
        }
        for good in [
            "localhost:4393",
            "[::1]:4393",
            "127.0.0.1",
            "kanade.example",
        ] {
            let mut input = values();
            input.insert("KANADE_ADMIN_HOST".into(), good.into());
            assert!(RuntimeConfig::from_mapping(&input).is_ok(), "{good}");
        }
        assert_eq!(
            error_for(&[("KANADE_TRUSTED_PROXY", "edge.internal")]),
            "KANADE_TRUSTED_PROXY must be an IP address"
        );
    }

    #[test]
    fn healthcheck_rejects_non_loopback_urls() {
        let error = HealthcheckConfig::from_mapping(
            &BTreeMap::new(),
            Some("http://192.0.2.1:8080/healthz"),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "healthcheck target must be loopback");
    }

    #[test]
    fn private_binds_need_the_opt_in_and_never_include_wildcards_or_public_addresses() {
        assert_eq!(
            error_for(&[("KANADE_ADMIN_BIND", "172.18.0.5:8080")]),
            "KANADE_ADMIN_BIND must be a loopback address"
        );
        for good in [
            "172.18.0.5:8080",
            "10.1.2.3:8080",
            "192.168.1.9:80",
            "[fd00::5]:8080",
            "[fe80::1]:80",
            "127.0.0.1:8080",
        ] {
            let mut input = values();
            input.insert("KANADE_ALLOW_PRIVATE_BIND".into(), "1".into());
            input.insert("KANADE_ADMIN_BIND".into(), good.into());
            assert!(RuntimeConfig::from_mapping(&input).is_ok(), "{good}");
        }
        for bad in [
            "0.0.0.0:8080",
            "[::]:8080",
            "203.0.113.4:8080",
            "[2001:db8::1]:80",
            "[::ffff:10.0.0.1]:80",
        ] {
            assert_eq!(
                error_for(&[
                    ("KANADE_ALLOW_PRIVATE_BIND", "1"),
                    ("KANADE_ADMIN_BIND", bad)
                ]),
                "KANADE_ADMIN_BIND must be a loopback or private address",
                "{bad}"
            );
        }
        assert_eq!(
            error_for(&[("KANADE_ALLOW_PRIVATE_BIND", "yes")]),
            "KANADE_ALLOW_PRIVATE_BIND must be 0 or 1"
        );
        let private = BTreeMap::from([("KANADE_ALLOW_PRIVATE_BIND".into(), "1".into())]);
        assert!(
            HealthcheckConfig::from_mapping(&private, Some("http://172.18.0.5:8080/healthz"))
                .is_ok()
        );
        assert!(
            HealthcheckConfig::from_mapping(&private, Some("http://203.0.113.4:8080/healthz"))
                .is_err()
        );
    }

    #[test]
    fn edge_secret_needs_a_trusted_proxy() {
        assert_eq!(
            error_for(&[("KANADE_EDGE_SECRET_FILE", "/run/secrets/edge")]),
            "KANADE_EDGE_SECRET_FILE requires KANADE_TRUSTED_PROXY"
        );
    }
}
