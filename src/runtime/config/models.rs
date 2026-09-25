//! Model gateway settings. The key is read from a file; the URL follows the
//! provider's rule (https, or http only to loopback, `localhost` or
//! `host.docker.internal`) so a bad value fails at startup, not first call.

use std::{collections::BTreeMap, net::IpAddr, path::PathBuf};

use hyper::Uri;

use super::{Error, non_empty, parse_bounded_u64};
use crate::runtime::secrets::Redacted;

const KEY_FILE: &str = "KANADE_MODEL_KEY_FILE";
const ALIASES: [&str; 3] = [
    "KANADE_EXTRACT_MODEL",
    "KANADE_CHAT_MODEL",
    "KANADE_REWRITE_MODEL",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSettings {
    /// Unset disables every model feature.
    pub base_url: Option<String>,
    pub key_file: Option<PathBuf>,
    pub ca_file: Option<PathBuf>,
    pub extract_model: Option<String>,
    pub chat_model: Option<String>,
    pub rewrite_model: Option<String>,
    pub permits: u16,
}

impl ModelSettings {
    pub(super) fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
        if non_empty(values, "KANADE_MODEL_KEY").is_some() {
            return Err(Error::Configuration(format!(
                "KANADE_MODEL_KEY is not read; use {KEY_FILE}"
            )));
        }
        let base_url = non_empty(values, "KANADE_MODEL_BASE_URL")
            .map(|url| {
                valid_base_url(url).then(|| url.to_owned()).ok_or_else(|| {
                    Error::Configuration(
                        "KANADE_MODEL_BASE_URL must be https, or http to a loopback host".into(),
                    )
                })
            })
            .transpose()?;
        let [extract_model, chat_model, rewrite_model] = ALIASES.map(|key| alias(values, key));
        let settings = Self {
            key_file: non_empty(values, KEY_FILE).map(PathBuf::from),
            ca_file: non_empty(values, "KANADE_MODEL_CA_FILE").map(PathBuf::from),
            extract_model: extract_model?,
            chat_model: chat_model?,
            rewrite_model: rewrite_model?,
            permits: parse_bounded_u64(values, "KANADE_MODEL_PERMITS", 2, 1, 16)? as u16,
            base_url,
        };
        if settings.base_url.is_none() {
            let dependent = [KEY_FILE, "KANADE_MODEL_CA_FILE"]
                .into_iter()
                .chain(ALIASES)
                .find(|key| non_empty(values, key).is_some());
            if let Some(key) = dependent {
                return Err(Error::Configuration(format!(
                    "{key} requires KANADE_MODEL_BASE_URL"
                )));
            }
        }
        Ok(settings)
    }

    pub fn read_key(&self) -> Result<Option<Redacted>, Error> {
        self.key_file
            .as_deref()
            .map(|path| Redacted::read(path, KEY_FILE))
            .transpose()
    }
}

fn alias(values: &BTreeMap<String, String>, key: &str) -> Result<Option<String>, Error> {
    non_empty(values, key)
        .map(|name| {
            let ok = name.len() <= 200 && name.bytes().all(|byte| byte.is_ascii_graphic());
            ok.then(|| name.to_owned())
                .ok_or_else(|| Error::Configuration(format!("{key} must be a model alias")))
        })
        .transpose()
}

/// Mirrors `infrastructure::llm` endpoint parsing.
fn valid_base_url(url: &str) -> bool {
    let Ok(uri) = url.parse::<Uri>() else {
        return false;
    };
    let Some(authority) = uri.authority() else {
        return false;
    };
    if authority.as_str().contains('@') || uri.query().is_some() {
        return false;
    }
    let raw = authority.host();
    let host = raw
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(raw)
        .to_ascii_lowercase();
    match uri.scheme_str() {
        Some("https") => !host.is_empty(),
        Some("http") => {
            host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
                || host == "localhost"
                || host == "host.docker.internal"
        }
        _ => false,
    }
}
