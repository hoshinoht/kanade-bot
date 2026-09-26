//! Model gateway settings. The key is read from a file; the URL follows the
//! provider's rule (https, or http only to loopback, `localhost` or
//! `host.docker.internal`) so a bad value fails at startup, not first call.

use std::{collections::BTreeMap, net::IpAddr, path::PathBuf};

use hyper::Uri;

use super::{Error, groups, non_empty, parse_bounded_u64};
use crate::{
    domain::settings::Reasoning, infrastructure::llm::setup::CapacityGroup,
    runtime::secrets::Redacted,
};

const KEY_FILE: &str = "KANADE_MODEL_KEY_FILE";
const UNMASKED: &str = "KANADE_ALLOW_EXTERNAL_UNMASKED";
const PSEUDONYMIZE: &str = "KANADE_PSEUDONYMIZE";
const ALIASES: [&str; 3] = [
    "KANADE_EXTRACT_MODEL",
    "KANADE_CHAT_MODEL",
    "KANADE_REWRITE_MODEL",
];
const REASONING: [&str; 3] = [
    "KANADE_EXTRACT_REASONING",
    "KANADE_CHAT_REASONING",
    "KANADE_REWRITE_REASONING",
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
    /// Seeds for the stored `extract_reasoning`, `chat_pilot_think` and
    /// `v5.rewrite_reasoning`: used only where no row is saved.
    pub extract_reasoning: Option<Reasoning>,
    pub chat_reasoning: Option<Reasoning>,
    pub rewrite_reasoning: Option<Reasoning>,
    pub permits: u16,
    /// Non-empty replaces the single gateway group of `permits`.
    pub groups: Vec<CapacityGroup>,
    /// `KANADE_ALLOW_EXTERNAL_UNMASKED=1`: external routes may run without
    /// pseudonymization (provider testing only).
    pub allow_external_unmasked: bool,
    /// `KANADE_PSEUDONYMIZE=1` (`models.pseudonymize`): member identities
    /// reach every model role as per-request fictional names. Default off.
    pub pseudonymize: bool,
}

impl ModelSettings {
    pub fn from_mapping(values: &BTreeMap<String, String>) -> Result<Self, Error> {
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
        let [extract_reasoning, chat_reasoning, rewrite_reasoning] =
            REASONING.map(|key| reasoning(values, key));
        let settings = Self {
            key_file: non_empty(values, KEY_FILE).map(PathBuf::from),
            ca_file: non_empty(values, "KANADE_MODEL_CA_FILE").map(PathBuf::from),
            extract_model: extract_model?,
            chat_model: chat_model?,
            rewrite_model: rewrite_model?,
            extract_reasoning: extract_reasoning?,
            chat_reasoning: chat_reasoning?,
            rewrite_reasoning: rewrite_reasoning?,
            permits: parse_bounded_u64(values, "KANADE_MODEL_PERMITS", 2, 1, 16)? as u16,
            groups: groups::parse(values)?,
            allow_external_unmasked: flag(values, UNMASKED)?,
            pseudonymize: flag(values, PSEUDONYMIZE)?,
            base_url,
        };
        if settings.base_url.is_none() {
            let dependent = [KEY_FILE, "KANADE_MODEL_CA_FILE", groups::GROUPS]
                .into_iter()
                .chain(ALIASES)
                .chain(REASONING)
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

fn flag(values: &BTreeMap<String, String>, key: &str) -> Result<bool, Error> {
    match non_empty(values, key) {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(_) => Err(Error::Configuration(format!("{key} must be 0 or 1"))),
    }
}

/// `off`…`max`; `inherit` (extraction's level) for chat and rewrite only.
fn reasoning(values: &BTreeMap<String, String>, key: &str) -> Result<Option<Reasoning>, Error> {
    let Some(text) = non_empty(values, key) else {
        return Ok(None);
    };
    let level = if text == "inherit" && key != REASONING[0] {
        Some(Reasoning::Inherit)
    } else {
        Reasoning::parse(text)
            .filter(|level| *level != Reasoning::Inherit && level.as_str() == text)
    };
    level.map(Some).ok_or_else(|| {
        let allowed = if key == REASONING[0] {
            "off, minimal, low, medium, high, xhigh or max"
        } else {
            "inherit, off, minimal, low, medium, high, xhigh or max"
        };
        Error::Configuration(format!("{key} must be one of {allowed}"))
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(pairs: &[(&str, &str)]) -> Result<ModelSettings, String> {
        let mut values: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        values
            .entry("KANADE_MODEL_BASE_URL".into())
            .or_insert_with(|| "https://gw.example/v1".into());
        ModelSettings::from_mapping(&values).map_err(|error| error.to_string())
    }

    #[test]
    fn pseudonymize_is_an_off_by_default_flag() {
        assert!(!parse(&[]).unwrap().pseudonymize);
        assert!(parse(&[("KANADE_PSEUDONYMIZE", "1")]).unwrap().pseudonymize);
        assert_eq!(
            parse(&[("KANADE_PSEUDONYMIZE", "yes")]).unwrap_err(),
            "KANADE_PSEUDONYMIZE must be 0 or 1"
        );
    }

    #[test]
    fn reasoning_seeds_parse_and_refuse() {
        let settings = parse(&[
            ("KANADE_EXTRACT_REASONING", "low"),
            ("KANADE_CHAT_REASONING", "inherit"),
            ("KANADE_REWRITE_REASONING", "off"),
        ])
        .unwrap();
        assert_eq!(settings.extract_reasoning, Some(Reasoning::Low));
        assert_eq!(settings.chat_reasoning, Some(Reasoning::Inherit));
        assert_eq!(settings.rewrite_reasoning, Some(Reasoning::Off));
        assert_eq!(parse(&[]).unwrap().chat_reasoning, None);
        assert_eq!(
            parse(&[("KANADE_EXTRACT_REASONING", "inherit")]).unwrap_err(),
            "KANADE_EXTRACT_REASONING must be one of off, minimal, low, medium, high, xhigh or max"
        );
        for bad in ["none", "High", "loud", "false"] {
            assert_eq!(
                parse(&[("KANADE_CHAT_REASONING", bad)]).unwrap_err(),
                "KANADE_CHAT_REASONING must be one of inherit, off, minimal, low, medium, high, xhigh or max",
                "{bad}"
            );
        }
        let mut orphan = BTreeMap::new();
        orphan.insert("KANADE_REWRITE_REASONING".to_owned(), "low".to_owned());
        assert_eq!(
            ModelSettings::from_mapping(&orphan)
                .unwrap_err()
                .to_string(),
            "KANADE_REWRITE_REASONING requires KANADE_MODEL_BASE_URL"
        );
    }
}
