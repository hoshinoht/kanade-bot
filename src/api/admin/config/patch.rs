//! `ConfigPatch`: one section per request, partial, arrays replaced whole.
//! Unknown keys are `422 unknown_field`, read-only ones `422 read_only`,
//! and values the section cannot take `422 invalid`. Values are normalised
//! to stored form here, so `save_section` never sees an unrepresentable one.

use axum::http::StatusCode;
use chrono::NaiveTime;
use serde_json::{Map, Value};

use crate::{
    api::admin::write::Refusal,
    domain::settings::{
        Chatbot, Notifications, Pings, Rate, SelfService, SelfServiceMode, Watching,
    },
};

pub const MAX_COUNTDOWNS: usize = 4;
pub const COUNTDOWN_MINUTES: std::ops::RangeInclusive<u64> = 5..=24 * 60;
pub const RATE_COUNT: std::ops::RangeInclusive<u64> = 1..=100;
pub const RATE_WINDOW_S: std::ops::RangeInclusive<u64> = 10..=86_400;

#[derive(Debug, PartialEq, Eq)]
pub struct PatchError(pub Refusal);

impl PatchError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self(Refusal::invalid(message))
    }

    pub fn unknown(path: impl AsRef<str>) -> Self {
        Self(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_field",
            format!("Unknown setting: {}.", path.as_ref()),
        ))
    }

    pub fn read_only(path: impl AsRef<str>, why: &str) -> Self {
        Self(Refusal::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "read_only",
            format!("{} cannot be changed here: {why}", path.as_ref()),
        ))
    }
}

impl From<PatchError> for Refusal {
    fn from(error: PatchError) -> Self {
        error.0
    }
}

pub fn field_error(path: &str, expected: &str) -> PatchError {
    PatchError::invalid(format!("{path} must be {expected}."))
}

pub fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, PatchError> {
    value
        .as_object()
        .ok_or_else(|| field_error(path, "an object"))
}

const DERIVED: &str = "it is derived by the server.";
const DEPLOYMENT: &str = "it is set by the deployment.";
const NOT_STORED: &str = "saving it is not supported yet.";
const GROUPS: &str = "it is set in kanade.toml ([[models.groups]]); restart to apply.";

/// Read-only keys the view carries; anything else not writable is unknown.
fn read_only(section: &str, key: &str) -> Option<&'static str> {
    match (section, key) {
        ("chatbot", "configured" | "missing_env")
        | ("self_service", "effective_mode")
        | (
            "models",
            "reachable" | "catalog" | "groups_source" | "alias_limits" | "capacity_check",
        )
        | ("persona", "personas" | "profiles") => Some(DERIVED),
        ("models", "key_limits" | "pii_pseudonymise") => Some(DEPLOYMENT),
        // Contracted as editable, but neither the settings port nor the
        // governor can hold them yet.
        ("models", "groups") => Some(GROUPS),
        ("persona", "role_profiles" | "visibility") => Some(NOT_STORED),
        _ => None,
    }
}

/// The one section a request names, with its body.
pub fn section(body: &Value) -> Result<(&str, &Map<String, Value>), PatchError> {
    let fields = body
        .as_object()
        .ok_or_else(|| PatchError::invalid("Send one settings section."))?;
    let mut names = fields.keys();
    let (Some(name), None) = (names.next(), names.next()) else {
        return Err(PatchError::invalid(if fields.is_empty() {
            "Send one settings section."
        } else {
            "Save one section at a time."
        }));
    };
    match name.as_str() {
        "pings" | "watching" | "chatbot" | "notifications" | "self_service" | "persona"
        | "models" => {}
        "manage_messages" | "env" | "notices" => {
            return Err(PatchError::read_only(name, DEPLOYMENT));
        }
        other => return Err(PatchError::unknown(other)),
    }
    let body = object(&fields[name], name)?;
    if body.is_empty() {
        return Err(PatchError::invalid(format!(
            "Send at least one {name} setting."
        )));
    }
    for key in body.keys() {
        let writable = matches!(
            (name.as_str(), key.as_str()),
            ("pings", "day_of_ping_time" | "countdown_minutes")
                | ("watching", "paused" | "extract_enabled")
                | ("chatbot", "enabled" | "member_rate" | "guild_rate")
                | ("notifications", "quiet_mode")
                | ("self_service", "mode" | "public_portal")
                | ("persona", "active")
                | ("models", "roles")
        );
        if writable {
            continue;
        }
        let path = format!("{name}.{key}");
        return Err(match read_only(name, key) {
            Some(why) => PatchError::read_only(path, why),
            None => PatchError::unknown(path),
        });
    }
    Ok((name, body))
}

fn flag(value: &Value, path: &str) -> Result<bool, PatchError> {
    value
        .as_bool()
        .ok_or_else(|| field_error(path, "true or false"))
}

/// Exactly `HH:MM`, 24-hour.
fn clock(value: &Value) -> Result<NaiveTime, PatchError> {
    let bad = || PatchError::invalid("The morning ping is HH:MM, for example 09:00.");
    let text = value.as_str().ok_or_else(bad)?.trim();
    let bytes = text.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return Err(bad());
    }
    let part = |range: std::ops::Range<usize>| {
        text[range.clone()]
            .bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| text[range].parse::<u32>().ok())
            .flatten()
    };
    part(0..2)
        .zip(part(3..5))
        .and_then(|(hour, minute)| NaiveTime::from_hms_opt(hour, minute, 0))
        .ok_or_else(bad)
}

/// Largest first, no duplicates (stored form).
fn countdowns(value: &Value) -> Result<Vec<u32>, PatchError> {
    let bad = || {
        PatchError::invalid(format!(
            "Countdowns are up to {MAX_COUNTDOWNS} whole minutes between {} and {}.",
            COUNTDOWN_MINUTES.start(),
            COUNTDOWN_MINUTES.end()
        ))
    };
    let list = value.as_array().ok_or_else(bad)?;
    let mut minutes = list
        .iter()
        .map(|item| {
            item.as_u64()
                .filter(|minute| COUNTDOWN_MINUTES.contains(minute))
                .map(|minute| minute as u32)
                .ok_or_else(bad)
        })
        .collect::<Result<Vec<_>, _>>()?;
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    minutes.dedup();
    if minutes.len() > MAX_COUNTDOWNS {
        return Err(bad());
    }
    Ok(minutes)
}

fn rate(current: Rate, value: &Value, path: &str) -> Result<Rate, PatchError> {
    let bad = || {
        PatchError::invalid(format!(
            "A rate is {}-{} answers per {}-{} seconds.",
            RATE_COUNT.start(),
            RATE_COUNT.end(),
            RATE_WINDOW_S.start(),
            RATE_WINDOW_S.end()
        ))
    };
    let mut next = current;
    for (key, value) in object(value, path)? {
        let (slot, range) = match key.as_str() {
            "count" => (&mut next.count, RATE_COUNT),
            "window_s" => (&mut next.window_s, RATE_WINDOW_S),
            other => return Err(PatchError::unknown(format!("{path}.{other}"))),
        };
        *slot = value
            .as_u64()
            .filter(|number| range.contains(number))
            .ok_or_else(bad)? as u32;
    }
    Ok(next)
}

pub fn pings(current: &Pings, body: &Map<String, Value>) -> Result<Pings, PatchError> {
    let mut next = current.clone();
    if let Some(value) = body.get("day_of_ping_time") {
        next.day_of_ping_time = clock(value)?;
    }
    if let Some(value) = body.get("countdown_minutes") {
        next.countdown_minutes = countdowns(value)?;
    }
    Ok(next)
}

pub fn watching(current: &Watching, body: &Map<String, Value>) -> Result<Watching, PatchError> {
    let mut next = current.clone();
    if let Some(value) = body.get("paused") {
        next.paused = flag(value, "watching.paused")?;
    }
    if let Some(value) = body.get("extract_enabled") {
        next.extract_enabled = flag(value, "watching.extract_enabled")?;
    }
    Ok(next)
}

/// `missing_env` non-empty: the chatbot cannot be turned on.
pub fn chatbot(
    current: &Chatbot,
    body: &Map<String, Value>,
    missing_env: &[String],
) -> Result<Chatbot, PatchError> {
    let mut next = current.clone();
    if let Some(value) = body.get("enabled") {
        next.enabled = flag(value, "chatbot.enabled")?;
        if next.enabled && !current.enabled && !missing_env.is_empty() {
            return Err(PatchError::invalid(format!(
                "The chatbot needs {} set first.",
                missing_env.join(", ")
            )));
        }
    }
    if let Some(value) = body.get("member_rate") {
        next.member_rate = rate(current.member_rate, value, "chatbot.member_rate")?;
    }
    if let Some(value) = body.get("guild_rate") {
        next.guild_rate = rate(current.guild_rate, value, "chatbot.guild_rate")?;
    }
    Ok(next)
}

pub fn notifications(body: &Map<String, Value>) -> Result<Notifications, PatchError> {
    Ok(Notifications {
        quiet_mode: flag(&body["quiet_mode"], "notifications.quiet_mode")?,
    })
}

pub fn self_service(
    current: &SelfService,
    body: &Map<String, Value>,
) -> Result<SelfService, PatchError> {
    let mut next = *current;
    if let Some(value) = body.get("mode") {
        next.mode = value
            .as_str()
            .and_then(SelfServiceMode::parse)
            .ok_or_else(|| {
                PatchError::invalid("Self-service is cards_and_link, link_first or cards_only.")
            })?;
    }
    if let Some(value) = body.get("public_portal") {
        next.public_portal = flag(value, "self_service.public_portal")?;
    }
    Ok(next)
}

/// `persona.active`: a persona id (validated against the files by the caller).
pub fn persona_active(body: &Map<String, Value>) -> Result<String, PatchError> {
    body["active"]
        .as_str()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| PatchError::invalid("Pick a persona from the catalog."))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn code(body: Value) -> &'static str {
        section(&body).unwrap_err().0.error
    }

    #[test]
    fn one_known_section_with_writable_keys_only() {
        assert_eq!(code(json!({})), "invalid");
        assert_eq!(code(json!([])), "invalid");
        assert_eq!(
            code(json!({"pings": {"day_of_ping_time": "09:00"}, "watching": {"paused": true}})),
            "invalid"
        );
        assert_eq!(code(json!({"nope": {}})), "unknown_field");
        assert_eq!(code(json!({"env": []})), "read_only");
        assert_eq!(code(json!({"pings": {}})), "invalid");
        assert_eq!(code(json!({"pings": {"zone": "x"}})), "unknown_field");
        assert_eq!(
            code(json!({"self_service": {"effective_mode": "cards_only"}})),
            "read_only"
        );
        assert_eq!(code(json!({"models": {"groups": []}})), "read_only");
        assert!(section(&json!({"watching": {"paused": true}})).is_ok());
    }

    #[test]
    fn countdowns_are_bounded_and_normalised() {
        let current = Pings::default();
        let next = pings(
            &current,
            object(&json!({"countdown_minutes": [15, 60, 15]}), "p").unwrap(),
        )
        .unwrap();
        assert_eq!(next.countdown_minutes, [60, 15]);
        for bad in [
            json!([4]),
            json!([5, 10, 15, 20, 25]),
            json!(["5"]),
            json!(5),
        ] {
            let body = json!({ "countdown_minutes": bad });
            assert!(
                pings(&current, object(&body, "p").unwrap()).is_err(),
                "{bad}"
            );
        }
        for bad in ["9:00", "24:00", "09:60", "0900", "ab:cd"] {
            let body = json!({ "day_of_ping_time": bad });
            assert!(
                pings(&current, object(&body, "p").unwrap()).is_err(),
                "{bad}"
            );
        }
    }
}
