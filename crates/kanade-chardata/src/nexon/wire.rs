//! The Open API response shapes kanade reads (SEA character spec, revised
//! 2025-12-18). Only the fields kept are decoded; everything else in a body
//! (guild, gender, exp, creation date, access flag, …) is ignored and never
//! stored. Every field is optional: an empty response (a character with no
//! data at the date) is all `null`.

use serde::Deserialize;

/// The `final_stat` entry holding combat power: confirmed on live MapleSEA
/// (2026-10-12) as `"Combat Power"` with plain digits (e.g. `"123456789"`).
/// Matching ignores case and surrounding spaces.
pub const COMBAT_POWER_STAT: &str = "Combat Power";

#[derive(Deserialize)]
pub(super) struct IdBody {
    pub ocid: Option<String>,
}

/// `/character/basic`.
#[derive(Debug, Default, Deserialize)]
pub struct Basic {
    pub date: Option<String>,
    pub character_name: Option<String>,
    pub world_name: Option<String>,
    pub character_class: Option<String>,
    pub character_level: Option<i64>,
    pub character_image: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct StatBody {
    pub final_stat: Option<Vec<StatEntry>>,
}

#[derive(Deserialize)]
pub(super) struct StatEntry {
    pub stat_name: Option<String>,
    pub stat_value: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ErrorBody {
    pub error: Option<ErrorName>,
}

#[derive(Deserialize)]
pub(super) struct ErrorName {
    pub name: Option<String>,
}

/// Combat power from a `final_stat` list: the [`COMBAT_POWER_STAT`] entry's
/// value as a whole number, digit-group separators allowed; `None` when
/// absent or not a whole number.
pub(super) fn combat_power(stats: &[StatEntry]) -> Option<u64> {
    let entry = stats.iter().find(|entry| {
        entry
            .stat_name
            .as_deref()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case(COMBAT_POWER_STAT))
    })?;
    let digits: String = entry
        .stat_value
        .as_deref()?
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | ' ' | '_'))
        .collect();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// An `OPENAPI000NN` error name, else `None` (never the message).
pub(super) fn error_code(body: &[u8]) -> Option<String> {
    let name = serde_json::from_slice::<ErrorBody>(body)
        .ok()?
        .error?
        .name?;
    let valid = name.len() == 12
        && name.starts_with("OPENAPI")
        && name[7..].bytes().all(|byte| byte.is_ascii_digit());
    valid.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, value: &str) -> StatEntry {
        StatEntry {
            stat_name: Some(name.into()),
            stat_value: Some(value.into()),
        }
    }

    #[test]
    fn combat_power_is_read_tolerantly() {
        let stats = [
            entry("Max HP", "99999"),
            entry(" combat power ", "12,345,678"),
        ];
        assert_eq!(combat_power(&stats), Some(12_345_678));
        assert_eq!(combat_power(&[entry("Combat Power", "48.2M")]), None);
        assert_eq!(combat_power(&[entry("Combat Power", "")]), None);
        assert_eq!(combat_power(&[entry("Max HP", "1")]), None);
        assert_eq!(combat_power(&[]), None);
    }

    #[test]
    fn only_wellformed_error_codes_are_kept() {
        let body = br#"{"error":{"name":"OPENAPI00007","message":"too many"}}"#;
        assert_eq!(error_code(body).as_deref(), Some("OPENAPI00007"));
        assert_eq!(error_code(br#"{"error":{"name":"<script>"}}"#), None);
        assert_eq!(error_code(b"not json"), None);
    }
}
