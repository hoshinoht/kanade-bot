//! `ConfigPatch`: one section per request, partial; arrays replace whole except
//! for the selected-key `persona.visibility` delta.
//! Unknown keys are `422 unknown_field`, read-only ones `422 read_only`,
//! and values the section cannot take `422 invalid`. Values are normalised
//! to stored form here, so `save_section` never sees an unrepresentable one.

use axum::http::StatusCode;
use chrono::NaiveTime;
use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::{
    api::admin::write::Refusal,
    chat::{
        nudge::{WordFilter, is_builtin_word},
        persona::ProfileId,
    },
    domain::settings::{
        Chatbot, ContextSettings, IdList, MAX_DEFLECTION_CHARS, MAX_PROFANITY_WORDS,
        MAX_ROLE_PROFILE_ASSIGNMENTS, Notifications, OVERRIDE_RUN_MINUTES, PROFANITY_WORD_CHARS,
        Persona, Pings, Profanity, RUN_MINUTES, Rate, RoleProfileAssignment, RunLengthOverride,
        RunLengths, SelfService, SelfServiceMode, Watching, is_profanity_word,
    },
};

pub const MAX_COUNTDOWNS: usize = 4;
pub const COUNTDOWN_MINUTES: std::ops::RangeInclusive<u64> = 5..=24 * 60;
pub const RATE_COUNT: std::ops::RangeInclusive<u64> = 1..=100;
/// 0 = staff only: members without an override are not answered.
pub const MEMBER_RATE_COUNT: std::ops::RangeInclusive<u64> = 0..=100;
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
const GROUPS: &str = "it is set in kanade.toml ([[models.groups]]); restart to apply.";

/// Read-only keys the view carries; anything else not writable is unknown.
fn read_only(section: &str, key: &str) -> Option<&'static str> {
    match (section, key) {
        ("chatbot", "configured" | "missing_env" | "category_ids_source")
        | ("watching", "channel_ids_source" | "category_ids_source")
        | ("self_service", "effective_mode")
        | (
            "models",
            "reachable" | "catalog" | "groups_source" | "alias_limits" | "capacity_check",
        )
        | ("persona", "personas" | "profiles" | "role_profiles_digest")
        | ("profanity", "builtin_words") => Some(DERIVED),
        ("models", "key_limits" | "pii_pseudonymise") => Some(DEPLOYMENT),
        ("models", "groups") => Some(GROUPS),
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
        | "models" | "run_lengths" | "profanity" => {}
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
                | (
                    "watching",
                    "paused" | "extract_enabled" | "channel_ids" | "category_ids"
                )
                | (
                    "chatbot",
                    "enabled" | "member_rate" | "guild_rate" | "category_ids"
                )
                | ("notifications", "quiet_mode")
                | ("self_service", "mode" | "public_portal")
                | ("persona", "active" | "visibility" | "role_profiles")
                | ("models", "roles" | "context")
                | ("run_lengths", "default_minutes" | "overrides")
                | (
                    "profanity",
                    "extra_words"
                        | "allowed_words"
                        | "check_questions"
                        | "check_replies"
                        | "deflection_line"
                )
        ) || (name == "persona"
            && key == "role_profiles_digest"
            && body.contains_key("role_profiles"));
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

/// At most this many ids per list.
pub const MAX_LIST_IDS: usize = 100;

/// The id list a `watching`/`chatbot` body names. A list is replaced only by
/// a body carrying it alone, so a toggle save never stores an env-seeded list.
pub fn id_list(name: &str, body: &Map<String, Value>) -> Result<Option<IdList>, PatchError> {
    let list = match name {
        "watching" if body.contains_key("channel_ids") => IdList::WatchedChannels,
        "watching" if body.contains_key("category_ids") => IdList::WatchedCategories,
        "chatbot" if body.contains_key("category_ids") => IdList::ChatCategories,
        _ => return Ok(None),
    };
    if body.len() != 1 {
        return Err(PatchError::invalid(
            "Save a channel or category list on its own.",
        ));
    }
    Ok(Some(list))
}

/// Canonical positive Discord ids, in the order given, no repeats.
pub fn ids(value: &Value, path: &str) -> Result<Vec<String>, PatchError> {
    let items = value
        .as_array()
        .ok_or_else(|| field_error(path, "an array of Discord ids"))?;
    if items.len() > MAX_LIST_IDS {
        return Err(PatchError::invalid(format!(
            "{path} holds at most {MAX_LIST_IDS} ids."
        )));
    }
    let mut ids: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .as_str()
            .filter(|id| canonical_role_id(id))
            .ok_or_else(|| field_error(path, "an array of canonical positive Discord ids"))?;
        if ids.iter().any(|seen| seen == id) {
            return Err(PatchError::invalid(format!(
                "{id} is listed twice in {path}."
            )));
        }
        ids.push(id.to_owned());
    }
    Ok(ids)
}

/// Context controls are saved as one complete JSON object, avoiding partial
/// values which could transiently pair a reserve with the wrong window.
pub fn context(value: &Value) -> Result<ContextSettings, PatchError> {
    serde_json::from_value(value.clone())
        .map_err(|_| field_error("models.context", "a complete context settings object"))
}

/// Overrides are replaced together because their `(boss, difficulty)` pair is
/// the stored identity, while the default may be patched on its own.
pub fn run_lengths(
    current: &RunLengths,
    body: &Map<String, Value>,
    catalog: &crate::domain::catalog::BossTable,
) -> Result<RunLengths, PatchError> {
    let mut next = current.clone();
    if let Some(value) = body.get("default_minutes") {
        next.default_minutes = value
            .as_u64()
            .and_then(|minutes| u32::try_from(minutes).ok())
            .filter(|minutes| RUN_MINUTES.contains(minutes))
            .ok_or_else(|| PatchError::invalid("The default run length is 5-240 whole minutes."))?;
    }
    if let Some(value) = body.get("overrides") {
        let values = value
            .as_array()
            .ok_or_else(|| field_error("run_lengths.overrides", "an array of overrides"))?;
        let mut overrides = Vec::with_capacity(values.len());
        let mut seen = BTreeSet::new();
        for (index, value) in values.iter().enumerate() {
            let path = format!("run_lengths.overrides[{index}]");
            let fields = object(value, &path)?;
            for key in fields.keys() {
                if !matches!(key.as_str(), "boss" | "difficulty" | "minutes") {
                    return Err(PatchError::unknown(format!("{path}.{key}")));
                }
            }
            let boss = fields
                .get("boss")
                .and_then(Value::as_str)
                .and_then(|key| catalog.boss(key).map(|boss| (key, boss)))
                .ok_or_else(|| PatchError::invalid("Pick a catalog boss key."))?;
            let difficulty = fields
                .get("difficulty")
                .and_then(Value::as_str)
                .filter(|difficulty| boss.1.difficulties().iter().any(|own| own == *difficulty))
                .ok_or_else(|| PatchError::invalid("Pick a difficulty that boss has."))?;
            let minutes = fields
                .get("minutes")
                .and_then(Value::as_u64)
                .and_then(|minutes| u32::try_from(minutes).ok())
                .filter(|minutes| OVERRIDE_RUN_MINUTES.contains(minutes))
                .ok_or_else(|| {
                    PatchError::invalid("An override run length is 5-480 whole minutes.")
                })?;
            if !seen.insert((boss.0, difficulty)) {
                return Err(PatchError::invalid(
                    "A boss difficulty can have only one run-length override.",
                ));
            }
            overrides.push(RunLengthOverride {
                boss: boss.0.to_owned(),
                difficulty: difficulty.to_owned(),
                minutes,
            });
        }
        next.overrides = overrides;
    }
    Ok(next)
}

/// A word list as stored: trimmed, lowercased, letters only, bounded, no
/// repeats (a repeat is refused rather than dropped, so nothing is silently lost).
fn words(value: &Value, path: &str) -> Result<Vec<String>, PatchError> {
    let items = value
        .as_array()
        .ok_or_else(|| field_error(path, "an array of words"))?;
    if items.len() > MAX_PROFANITY_WORDS {
        return Err(PatchError::invalid(format!(
            "{path} holds at most {MAX_PROFANITY_WORDS} words."
        )));
    }
    let mut words: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let word = item
            .as_str()
            .map(|word| word.trim().to_lowercase())
            .filter(|word| is_profanity_word(word))
            .ok_or_else(|| {
                PatchError::invalid(format!(
                    "{path} takes single words of {}-{} letters (no digits, spaces or symbols).",
                    PROFANITY_WORD_CHARS.start(),
                    PROFANITY_WORD_CHARS.end()
                ))
            })?;
        if words.contains(&word) {
            return Err(PatchError::invalid(format!(
                "“{word}” is listed twice in {path}."
            )));
        }
        words.push(word);
    }
    Ok(words)
}

/// The guardrail section merged onto `current`. Allowed words must be
/// built-ins; extra words must not be (allow-again is how a built-in comes
/// off); the deflection line must pass the list it is sent in place of.
pub fn profanity(current: &Profanity, body: &Map<String, Value>) -> Result<Profanity, PatchError> {
    let mut next = current.clone();
    if let Some(value) = body.get("extra_words") {
        next.extra_words = words(value, "profanity.extra_words")?;
    }
    if let Some(value) = body.get("allowed_words") {
        next.allowed_words = words(value, "profanity.allowed_words")?;
    }
    if let Some(value) = body.get("check_questions") {
        next.check_questions = flag(value, "profanity.check_questions")?;
    }
    if let Some(value) = body.get("check_replies") {
        next.check_replies = flag(value, "profanity.check_replies")?;
    }
    if let Some(value) = body.get("deflection_line") {
        let line = value
            .as_str()
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && line.chars().count() <= MAX_DEFLECTION_CHARS
                    && !line.chars().any(char::is_control)
            })
            .ok_or_else(|| {
                PatchError::invalid(format!(
                    "The deflection line is one line of 1-{MAX_DEFLECTION_CHARS} characters."
                ))
            })?;
        line.clone_into(&mut next.deflection_line);
    }
    if let Some(word) = next
        .allowed_words
        .iter()
        .find(|word| !is_builtin_word(word))
    {
        return Err(PatchError::invalid(format!(
            "“{word}” is not on the built-in list, so it cannot be allowed again."
        )));
    }
    if let Some(word) = next.extra_words.iter().find(|word| is_builtin_word(word)) {
        return Err(PatchError::invalid(format!(
            "“{word}” is already on the built-in list."
        )));
    }
    let filter = WordFilter::new(&next.extra_words, &next.allowed_words);
    if let Some(hit) = filter.denied(&next.deflection_line) {
        return Err(PatchError::invalid(format!(
            "The deflection line uses the listed word “{}”.",
            hit.as_str()
        )));
    }
    next.check().map_err(PatchError::invalid)?;
    Ok(next)
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

fn rate(
    current: Rate,
    value: &Value,
    path: &str,
    counts: std::ops::RangeInclusive<u64>,
) -> Result<Rate, PatchError> {
    let bad = || {
        PatchError::invalid(format!(
            "A rate is {}-{} answers per {}-{} seconds.",
            counts.start(),
            counts.end(),
            RATE_WINDOW_S.start(),
            RATE_WINDOW_S.end()
        ))
    };
    let mut next = current;
    for (key, value) in object(value, path)? {
        let (slot, range) = match key.as_str() {
            "count" => (&mut next.count, counts.clone()),
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
        next.member_rate = rate(
            current.member_rate,
            value,
            "chatbot.member_rate",
            MEMBER_RATE_COUNT,
        )?;
    }
    if let Some(value) = body.get("guild_rate") {
        next.guild_rate = rate(current.guild_rate, value, "chatbot.guild_rate", RATE_COUNT)?;
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

/// Parse the ordered assignments and their Config-view digest precondition.
pub fn role_profiles(
    body: &Map<String, Value>,
) -> Result<(Vec<RoleProfileAssignment>, String), PatchError> {
    if body.len() != 2 || !body.contains_key("role_profiles") {
        return Err(PatchError::invalid(
            "Save role_profiles with its digest, without other persona fields.",
        ));
    }
    let expected_digest = body
        .get("role_profiles_digest")
        .and_then(Value::as_str)
        .filter(|digest| !digest.is_empty())
        .ok_or_else(|| field_error("persona.role_profiles_digest", "a digest string"))?
        .to_owned();
    let items = body["role_profiles"].as_array().ok_or_else(|| {
        field_error(
            "persona.role_profiles",
            "an array of {role_id, profile} assignments",
        )
    })?;
    if items.len() > MAX_ROLE_PROFILE_ASSIGNMENTS {
        return Err(PatchError::invalid(format!(
            "Assign at most {MAX_ROLE_PROFILE_ASSIGNMENTS} Discord roles."
        )));
    }
    let mut assignments: Vec<RoleProfileAssignment> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let path = format!("persona.role_profiles[{index}]");
        let fields = object(item, &path)?;
        for field in fields.keys() {
            if !matches!(field.as_str(), "role_id" | "profile") {
                return Err(PatchError::unknown(format!("{path}.{field}")));
            }
        }
        let role_id = fields
            .get("role_id")
            .and_then(Value::as_str)
            .filter(|role_id| canonical_role_id(role_id))
            .ok_or_else(|| {
                field_error(
                    &format!("{path}.role_id"),
                    "a canonical positive Discord id",
                )
            })?;
        if assignments
            .iter()
            .any(|assignment| assignment.role_id == role_id)
        {
            return Err(PatchError::invalid(
                "A Discord role may be assigned only once.",
            ));
        }
        let profile = fields
            .get("profile")
            .and_then(Value::as_str)
            .ok_or_else(|| field_error(&format!("{path}.profile"), "a readable profile id"))?;
        ProfileId::parse(profile)
            .map_err(|_| PatchError::invalid("Pick a readable reply profile."))?;
        assignments.push(RoleProfileAssignment {
            role_id: role_id.to_owned(),
            profile: profile.to_owned(),
        });
    }
    Ok((assignments, expected_digest))
}

fn canonical_role_id(role_id: &str) -> bool {
    role_id
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .is_some_and(|id| id.to_string() == role_id)
}

/// Merge the selected-key visibility delta onto the settings locked by the
/// caller. Unknown or unreadable profiles cannot be published.
pub fn persona(
    current: &Persona,
    body: &Map<String, Value>,
    readable: &BTreeSet<ProfileId>,
) -> Result<Persona, PatchError> {
    let mut next = current.clone();
    if body.contains_key("active") {
        next.active = persona_active(body)?;
    }
    if let Some(value) = body.get("visibility") {
        let updates = value.as_array().ok_or_else(|| {
            field_error("persona.visibility", "an array of {key, public} changes")
        })?;
        let mut seen: Vec<String> = Vec::new();
        for (index, update) in updates.iter().enumerate() {
            let path = format!("persona.visibility[{index}]");
            let fields = object(update, &path)?;
            for field in fields.keys() {
                if !matches!(field.as_str(), "key" | "public") {
                    return Err(PatchError::unknown(format!("{path}.{field}")));
                }
            }
            let key = fields
                .get("key")
                .and_then(Value::as_str)
                .ok_or_else(|| field_error(&format!("{path}.key"), "a profile id"))?;
            let id = ProfileId::parse(key)
                .map_err(|_| PatchError::invalid("Pick a readable reply profile."))?;
            if !readable.contains(&id) {
                return Err(PatchError::invalid("Pick a readable reply profile."));
            }
            if seen.iter().any(|seen| seen == key) {
                return Err(PatchError::invalid(
                    "Each reply profile may appear only once per change.",
                ));
            }
            seen.push(key.to_owned());
            let public = fields
                .get("public")
                .and_then(Value::as_bool)
                .ok_or_else(|| field_error(&format!("{path}.public"), "true or false"))?;
            if public {
                if !next.profile_visibility.iter().any(|saved| saved == key) {
                    next.profile_visibility.push(key.to_owned());
                }
            } else {
                next.profile_visibility.retain(|saved| saved != key);
            }
        }
    }
    Ok(next)
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
        // The per-channel chat list was removed; it stays unknown.
        assert_eq!(
            code(json!({"chatbot": {"channel_ids": ["1"]}})),
            "unknown_field"
        );
    }

    #[test]
    fn a_list_is_saved_alone_as_canonical_unique_ids() {
        let body = |value: Value| value.as_object().unwrap().clone();
        assert_eq!(
            id_list("watching", &body(json!({"category_ids": []}))),
            Ok(Some(IdList::WatchedCategories))
        );
        assert_eq!(
            id_list("chatbot", &body(json!({"category_ids": []}))),
            Ok(Some(IdList::ChatCategories))
        );
        assert_eq!(
            id_list("watching", &body(json!({"paused": true}))),
            Ok(None)
        );
        assert_eq!(
            id_list(
                "watching",
                &body(json!({"paused": true, "channel_ids": []}))
            )
            .unwrap_err()
            .0
            .error,
            "invalid"
        );
        assert_eq!(ids(&json!(["12", "9"]), "p").unwrap(), ["12", "9"]);
        for bad in [
            json!("12"),
            json!([12]),
            json!(["012"]),
            json!(["0"]),
            json!(["12", "12"]),
            json!(["-1"]),
            Value::Array(vec![json!("1"); MAX_LIST_IDS + 1]),
        ] {
            assert!(ids(&bad, "p").is_err(), "{bad}");
        }
    }

    #[test]
    fn member_rate_may_be_zero_but_the_guild_pool_may_not() {
        let current = Chatbot::default();
        let staff_only = chatbot(
            &current,
            json!({"member_rate": {"count": 0}}).as_object().unwrap(),
            &[],
        )
        .expect("member count 0");
        assert_eq!(staff_only.member_rate.count, 0);
        let refused = chatbot(
            &current,
            json!({"guild_rate": {"count": 0}}).as_object().unwrap(),
            &[],
        );
        assert_eq!(refused.unwrap_err().0.error, "invalid");
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
