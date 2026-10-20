//! Module `character_data` (`modules.character_data` /
//! `KANADE_MODULE_CHARACTER_DATA`, default off, needs `modules.roster`):
//! `KANADE_CHARACTER_DATA_DB_PATH` (set by the image), the separate
//! character-data file (required while on), the optional `nexon_api_key` in
//! the secrets directory (absent: the module runs with links refused;
//! present but bad: serve refuses to start) and the optional
//! `character_data.scouter_url` / `KANADE_SCOUTER_URL` (the scouter
//! sidecar; unset: estimates are off). Off, all are ignored and no file is
//! opened.

use std::{collections::BTreeMap, net::IpAddr, path::PathBuf};

use hyper::Uri;

use super::{
    Error, is_private,
    layout::{CHARACTER_DATA_DB_PATH, absolute},
    non_empty,
};
use crate::runtime::secrets::{self, SecretFile};

pub const FLAG_KEY: &str = "KANADE_MODULE_CHARACTER_DATA";
pub const SCOUTER_URL_KEY: &str = "KANADE_SCOUTER_URL";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterDataSettings {
    pub db_path: PathBuf,
    /// `nexon_api_key`, read when present; `None` only where a caller turns
    /// the key off.
    pub nexon_key: Option<SecretFile>,
    /// `http://host[:port]` of the scouter sidecar; `None`: estimates
    /// are off.
    pub scouter_url: Option<String>,
}

impl CharacterDataSettings {
    /// `None` while the module is off.
    pub(super) fn from_mapping(
        values: &BTreeMap<String, String>,
        on: bool,
        roster: bool,
    ) -> Result<Option<Self>, Error> {
        if !on {
            return Ok(None);
        }
        if !roster {
            return Err(Error::Configuration(format!(
                "{FLAG_KEY} needs the roster module on (KANADE_MODULE_ROSTER)"
            )));
        }
        Ok(Some(Self {
            db_path: absolute(values, CHARACTER_DATA_DB_PATH)?,
            nexon_key: Some(SecretFile::named(values, secrets::NEXON_API_KEY)),
            scouter_url: non_empty(values, SCOUTER_URL_KEY)
                .map(|url| {
                    scouter_url(url).ok_or_else(|| {
                        Error::Configuration(format!(
                            "{SCOUTER_URL_KEY} must be http://host[:port] to a loopback, private or single-label host (e.g. http://scouter:8091)"
                        ))
                    })
                })
                .transpose()?,
        }))
    }
}

/// The sidecar has no authentication and carries roster IGNs, so it must be
/// on this host or a private network: plain `http` (no credentials, path,
/// query or fragment) to a loopback or private address, or a single-label
/// name such as a Compose service. Normalised to `http://host[:port]`.
fn scouter_url(url: &str) -> Option<String> {
    let uri = url.parse::<Uri>().ok()?;
    if uri.scheme_str() != Some("http") || uri.query().is_some() || url.contains('#') {
        return None;
    }
    if !matches!(uri.path(), "" | "/") {
        return None;
    }
    let authority = uri.authority()?;
    if authority.as_str().contains('@') {
        return None;
    }
    let raw = authority.host();
    let host = raw
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(raw)
        .to_ascii_lowercase();
    let allowed = match host.parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback() || is_private(ip),
        Err(_) => {
            (1..=63).contains(&host.len())
                && host
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !host.starts_with('-')
                && !host.ends_with('-')
        }
    };
    allowed.then(|| format!("http://{}", authority.as_str().to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::scouter_url;

    #[test]
    fn the_scouter_url_is_http_to_a_local_host_only() {
        for (url, normal) in [
            ("http://scouter:8091", "http://scouter:8091"),
            ("http://Scouter:8091/", "http://scouter:8091"),
            ("http://127.0.0.1:8091", "http://127.0.0.1:8091"),
            ("http://localhost", "http://localhost"),
            ("http://192.168.97.20:8091", "http://192.168.97.20:8091"),
            ("http://10.0.0.5:8091", "http://10.0.0.5:8091"),
            ("http://[::1]:8091", "http://[::1]:8091"),
            ("http://[fd00::5]:8091", "http://[fd00::5]:8091"),
        ] {
            assert_eq!(scouter_url(url).as_deref(), Some(normal), "{url}");
        }
        for url in [
            "https://scouter:8091",
            "http://scouter.example.com:8091",
            "http://8.8.8.8:8091",
            "http://[2001:db8::1]:8091",
            "http://user:pass@scouter:8091",
            "http://scouter:8091/v1",
            "http://scouter:8091/?ign=x",
            "http://scouter:8091/#x",
            "http://-scouter:8091",
            "scouter:8091",
            "",
        ] {
            assert_eq!(scouter_url(url), None, "{url}");
        }
    }
}
