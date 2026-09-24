use std::{collections::BTreeMap, net::SocketAddr, str::FromStr, time::Duration};

use chrono_tz::Tz;

use super::error::Error;

const DEFAULT_BIND: &str = "127.0.0.1:8080";
const DEFAULT_SHUTDOWN_SECONDS: u64 = 10;
const DEFAULT_HEALTHCHECK_TIMEOUT_SECONDS: u64 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub bind: SocketAddr,
    pub timezone: Tz,
    pub shutdown_timeout: Duration,
}

impl RuntimeConfig {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        let bind = value_or(values, "KANADE_BIND", DEFAULT_BIND)
            .parse::<SocketAddr>()
            .map_err(|_| {
                Error::Configuration("KANADE_BIND must be an IP address and port".into())
            })?;
        if !bind.ip().is_loopback() {
            return Err(Error::Configuration(
                "KANADE_BIND must be a loopback address".into(),
            ));
        }
        let timezone = values
            .get("KANADE_TIMEZONE")
            .ok_or_else(|| Error::Configuration("KANADE_TIMEZONE is required".into()))?
            .parse::<Tz>()
            .map_err(|_| {
                Error::Configuration("KANADE_TIMEZONE must be a valid IANA timezone".into())
            })?;

        Ok(Self {
            bind,
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
            target: parse_loopback_health_url(&target)?,
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

fn value_or<'a>(values: &'a BTreeMap<String, String>, key: &str, default: &'a str) -> &'a str {
    values.get(key).map_or(default, String::as_str)
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

fn parse_loopback_health_url(value: &str) -> Result<SocketAddr, Error> {
    let address = value
        .strip_prefix("http://")
        .and_then(|remainder| remainder.strip_suffix("/healthz"))
        .and_then(|address| SocketAddr::from_str(address).ok())
        .ok_or_else(|| {
            Error::Configuration("healthcheck URL must be http://LOOPBACK:PORT/healthz".into())
        })?;
    if !address.ip().is_loopback() {
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
        assert_eq!(config.bind.to_string(), DEFAULT_BIND);
        assert_eq!(config.timezone, chrono_tz::Asia::Kuala_Lumpur);
    }

    #[test]
    fn non_loopback_is_rejected_even_with_a_legacy_allowance() {
        let mut input = values();
        input.insert("KANADE_BIND".into(), "0.0.0.0:8080".into());
        input.insert("KANADE_ALLOW_NON_LOOPBACK".into(), "true".into());
        let error = RuntimeConfig::from_mapping(&input).unwrap_err();
        assert_eq!(error.to_string(), "KANADE_BIND must be a loopback address");
    }

    #[test]
    fn ipv6_non_loopback_is_rejected() {
        let mut input = values();
        input.insert("KANADE_BIND".into(), "[2001:db8::1]:8080".into());
        let error = RuntimeConfig::from_mapping(&input).unwrap_err();
        assert_eq!(error.to_string(), "KANADE_BIND must be a loopback address");
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
}
