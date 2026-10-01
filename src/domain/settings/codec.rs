//! Text encodings of the settings rows. Reads are strict: a value v4 never
//! wrote (or a v5 writer never would) is an error naming the key, so a
//! hand-edited row cannot silently change behaviour.

use std::collections::BTreeMap;

use chrono::{NaiveTime, Timelike, Weekday};

use super::SettingsError;
use super::keys;
use super::model::{
    Chatbot, ContextSettings, MAX_CONTEXT_TOKENS, MAX_ROLE_PROFILE_ASSIGNMENTS, Models,
    Notifications, Persona, Pings, Posting, Reasoning, RoleModel, RoleProfileAssignment,
    RuntimeSettings, Schedule, SelfService, SelfServiceMode, Watching,
};
use crate::domain::attendance::AttendanceMode;
use crate::domain::weeks::{parse_hhmm, parse_weekday};

/// One section written whole in one transaction (the config API's
/// `ConfigPatch` is merged onto the current section first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Section {
    Pings(Pings),
    Watching(Watching),
    Chatbot(Chatbot),
    Notifications(Notifications),
    SelfService(SelfService),
    Persona(Persona),
    Models(Models),
    Schedule(Schedule),
    Posting(Posting),
}

type Rows = Vec<(&'static str, String)>;

fn malformed(key: &'static str, value: &str, reason: impl Into<String>) -> SettingsError {
    SettingsError::Malformed {
        key,
        value: value.to_owned(),
        reason: reason.into(),
    }
}

/// v4 flags are exactly `1`/`0` (v4 reads anything but `1` as off).
fn flag(key: &'static str, value: &str) -> Result<bool, SettingsError> {
    match value {
        "1" => Ok(true),
        "0" => Ok(false),
        _ => Err(malformed(key, value, "expected 1 or 0")),
    }
}

fn flag_text(on: bool) -> String {
    if on { "1" } else { "0" }.to_owned()
}

fn clock(key: &'static str, value: &str) -> Result<NaiveTime, SettingsError> {
    parse_hhmm(value).map_err(|error| malformed(key, value, error.to_string()))
}

fn clock_text(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

fn weekday_text(day: Weekday) -> String {
    day.to_string().to_ascii_lowercase()
}

fn parts(value: &str) -> impl Iterator<Item = &str> {
    value
        .split([',', ';'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
}

fn countdowns(key: &'static str, value: &str) -> Result<Vec<u32>, SettingsError> {
    let mut minutes = parts(value)
        .map(|part| match part.parse::<u32>() {
            Ok(minute) if minute > 0 => Ok(minute),
            _ => Err(malformed(key, value, "expected positive whole minutes")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    minutes.dedup();
    Ok(minutes)
}

fn is_snowflake(id: &str) -> bool {
    id.bytes().all(|byte| byte.is_ascii_digit()) && id.parse::<u64>().is_ok_and(|id| id > 0)
}

fn ids(key: &'static str, value: &str) -> Result<Vec<String>, SettingsError> {
    parts(value)
        .map(|part| {
            if is_snowflake(part) {
                Ok(part.to_owned())
            } else {
                Err(malformed(key, value, "expected Discord ids"))
            }
        })
        .collect()
}

fn profile_ids(key: &'static str, value: &str) -> Result<Vec<String>, SettingsError> {
    let mut profiles = Vec::new();
    for profile in parts(value) {
        if !is_profile_id(profile) {
            return Err(malformed(key, value, "expected profile ids"));
        }
        if !profiles.iter().any(|existing| existing == profile) {
            profiles.push(profile.to_owned());
        }
    }
    Ok(profiles)
}

fn is_profile_id(profile: &str) -> bool {
    let mut chars = profile.bytes();
    matches!(chars.next(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && profile.len() <= 50
        && chars.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_canonical_role_id(role_id: &str) -> bool {
    role_id
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .is_some_and(|id| id.to_string() == role_id)
}

fn role_profiles(
    key: &'static str,
    value: &str,
) -> Result<Vec<RoleProfileAssignment>, SettingsError> {
    let assignments: Vec<RoleProfileAssignment> = serde_json::from_str(value).map_err(|_| {
        malformed(
            key,
            value,
            "expected an ordered JSON array of role/profile pairs",
        )
    })?;
    if assignments.len() > MAX_ROLE_PROFILE_ASSIGNMENTS {
        return Err(malformed(
            key,
            value,
            format!("expected at most {MAX_ROLE_PROFILE_ASSIGNMENTS} assignments"),
        ));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(assignments.len());
    for assignment in &assignments {
        if !is_canonical_role_id(&assignment.role_id) {
            return Err(malformed(
                key,
                value,
                "role ids must be canonical positive Discord ids",
            ));
        }
        if !is_profile_id(&assignment.profile) {
            return Err(malformed(key, value, "expected profile ids"));
        }
        if seen.contains(&assignment.role_id.as_str()) {
            return Err(malformed(key, value, "role ids must be unique"));
        }
        seen.push(&assignment.role_id);
    }
    Ok(assignments)
}

fn id(key: &'static str, value: &str) -> Result<Option<String>, SettingsError> {
    match value.trim() {
        "" => Ok(None),
        id if is_snowflake(id) => Ok(Some(id.to_owned())),
        _ => Err(malformed(key, value, "expected a Discord id")),
    }
}

/// `floor` 0 is the member allowance's "staff only"; the guild pool needs 1.
fn count(key: &'static str, value: &str, floor: u32) -> Result<u32, SettingsError> {
    match value.trim().parse::<u32>() {
        Ok(count) if count >= floor => Ok(count),
        _ => Err(malformed(
            key,
            value,
            format!("expected a whole number of at least {floor}"),
        )),
    }
}

/// v4 stored windows as float text (`300.0`); a fractional one rounds up, so
/// a limiter window never shortens.
fn window(key: &'static str, value: &str) -> Result<u32, SettingsError> {
    let seconds = value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s > 0.0);
    match seconds.map(f64::ceil) {
        // Finite, positive and in range, so the cast is exact.
        Some(seconds) if seconds <= f64::from(u32::MAX) => Ok(seconds as u32),
        _ => Err(malformed(
            key,
            value,
            "expected a positive number of seconds",
        )),
    }
}

fn reasoning(key: &'static str, value: &str, inherit: bool) -> Result<Reasoning, SettingsError> {
    match Reasoning::parse(value) {
        Some(Reasoning::Inherit) if !inherit => Err(malformed(
            key,
            value,
            "extraction cannot inherit a reasoning level",
        )),
        Some(level) => Ok(level),
        None => Err(malformed(key, value, "unknown reasoning level")),
    }
}

fn attendance_text(mode: AttendanceMode) -> &'static str {
    match mode {
        AttendanceMode::V4Compat => "v4_compat",
        AttendanceMode::V5 => "v5",
    }
}

/// `seed` with every stored settings row applied. A blank model alias row
/// is unset (v4 kept the seed), so the seed's alias stays.
pub(super) fn resolve(
    rows: &BTreeMap<String, String>,
    seed: &RuntimeSettings,
) -> Result<RuntimeSettings, SettingsError> {
    let mut out = seed.clone();
    for key in keys::ALL {
        if let Some(value) = rows.get(key) {
            apply(&mut out, key, value)?;
        }
    }
    Ok(out)
}

fn alias(value: &str) -> Option<String> {
    let alias = value.trim();
    (!alias.is_empty()).then(|| alias.to_owned())
}

fn context(key: &'static str, value: &str) -> Result<ContextSettings, SettingsError> {
    let context: ContextSettings = serde_json::from_str(value)
        .map_err(|_| malformed(key, value, "expected context settings JSON"))?;
    let valid_window = |window: u32| (1..=MAX_CONTEXT_TOKENS).contains(&window);
    if !valid_window(context.cloud_default) || !valid_window(context.local_default) {
        return Err(malformed(key, value, "defaults must be 1..=131072"));
    }
    for role in [&context.chat, &context.extraction, &context.rewrite] {
        if !valid_window(role.reserve) || role.cap.is_some_and(|cap| !valid_window(cap)) {
            return Err(malformed(
                key,
                value,
                "reserves and caps must be positive and at most 131072",
            ));
        }
    }
    if context
        .overrides
        .iter()
        .any(|(alias, window)| alias.trim().is_empty() || !valid_window(*window))
    {
        return Err(malformed(
            key,
            value,
            "overrides need model aliases and windows in 1..=131072",
        ));
    }
    Ok(context)
}

fn apply(out: &mut RuntimeSettings, key: &'static str, value: &str) -> Result<(), SettingsError> {
    match key {
        keys::DAY_OF_PING_TIME => out.pings.day_of_ping_time = clock(key, value)?,
        keys::COUNTDOWN_MINUTES => out.pings.countdown_minutes = countdowns(key, value)?,
        keys::PAUSED => out.watching.paused = flag(key, value)?,
        keys::EXTRACT_ENABLED => out.watching.extract_enabled = flag(key, value)?,
        keys::WATCHED_CHANNELS => out.watching.channel_ids = ids(key, value)?,
        keys::WATCHED_CATEGORIES => out.watching.category_ids = ids(key, value)?,
        keys::CHAT_MODE => out.chatbot.enabled = flag(key, value)?,
        keys::CHAT_CATEGORIES => out.chatbot.category_ids = ids(key, value)?,
        keys::CHAT_RATE_COUNT => out.chatbot.member_rate.count = count(key, value, 0)?,
        keys::CHAT_RATE_WINDOW => out.chatbot.member_rate.window_s = window(key, value)?,
        keys::CHAT_GLOBAL_RATE_COUNT => out.chatbot.guild_rate.count = count(key, value, 1)?,
        keys::CHAT_GLOBAL_RATE_WINDOW => out.chatbot.guild_rate.window_s = window(key, value)?,
        keys::QUIET_MODE => out.notifications.quiet_mode = flag(key, value)?,
        keys::SELF_SERVICE_MODE => {
            out.self_service.mode = SelfServiceMode::parse(value)
                .ok_or_else(|| malformed(key, value, "unknown self-service mode"))?;
        }
        keys::PUBLIC_PORTAL => out.self_service.public_portal = flag(key, value)?,
        keys::PERSONA => value.clone_into(&mut out.persona.active),
        keys::PROFILE_VISIBILITY => out.persona.profile_visibility = profile_ids(key, value)?,
        keys::ROLE_PROFILES => out.persona.role_profiles = role_profiles(key, value)?,
        keys::EXTRACT_MODEL | keys::CHAT_MODEL | keys::REWRITE_MODEL => {
            if let Some(alias) = alias(value) {
                role(&mut out.models, key).alias = Some(alias);
            }
        }
        keys::EXTRACT_REASONING => {
            out.models.extraction.reasoning = reasoning(key, value, false)?;
        }
        keys::CHAT_REASONING => out.models.chat.reasoning = reasoning(key, value, true)?,
        keys::REWRITE_REASONING => out.models.rewrite.reasoning = reasoning(key, value, true)?,
        keys::MODEL_CONTEXT => out.models.context = context(key, value)?,
        keys::RESET_WEEKDAY => {
            out.schedule.reset_weekday =
                parse_weekday(value).map_err(|error| malformed(key, value, error.to_string()))?;
        }
        keys::RESET_TIME => out.schedule.reset_time = clock(key, value)?,
        keys::ATTENDANCE_MODE => {
            out.schedule.attendance = [AttendanceMode::V4Compat, AttendanceMode::V5]
                .into_iter()
                .find(|mode| attendance_text(*mode) == value)
                .ok_or_else(|| malformed(key, value, "expected v4_compat or v5"))?;
        }
        keys::POST_CHANNEL => out.posting.channel_id = id(key, value)?,
        _ => {}
    }
    Ok(())
}

fn role<'a>(models: &'a mut Models, key: &str) -> &'a mut RoleModel {
    match key {
        keys::EXTRACT_MODEL => &mut models.extraction,
        keys::CHAT_MODEL => &mut models.chat,
        _ => &mut models.rewrite,
    }
}

fn list_text(ids: &[String]) -> String {
    ids.join(",")
}

/// The rows a section writes. `None` aliases write `""` (unset: the seed
/// applies, as in v4).
pub(super) fn encode(section: &Section) -> Rows {
    match section {
        Section::Pings(pings) => vec![
            (keys::DAY_OF_PING_TIME, clock_text(pings.day_of_ping_time)),
            (
                keys::COUNTDOWN_MINUTES,
                pings
                    .countdown_minutes
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        ],
        Section::Watching(watching) => vec![
            (keys::PAUSED, flag_text(watching.paused)),
            (keys::EXTRACT_ENABLED, flag_text(watching.extract_enabled)),
            (keys::WATCHED_CHANNELS, list_text(&watching.channel_ids)),
            (keys::WATCHED_CATEGORIES, list_text(&watching.category_ids)),
        ],
        Section::Chatbot(chat) => vec![
            (keys::CHAT_MODE, flag_text(chat.enabled)),
            (keys::CHAT_CATEGORIES, list_text(&chat.category_ids)),
            (keys::CHAT_RATE_COUNT, chat.member_rate.count.to_string()),
            (
                keys::CHAT_RATE_WINDOW,
                chat.member_rate.window_s.to_string(),
            ),
            (
                keys::CHAT_GLOBAL_RATE_COUNT,
                chat.guild_rate.count.to_string(),
            ),
            (
                keys::CHAT_GLOBAL_RATE_WINDOW,
                chat.guild_rate.window_s.to_string(),
            ),
        ],
        Section::Notifications(notifications) => {
            vec![(keys::QUIET_MODE, flag_text(notifications.quiet_mode))]
        }
        Section::SelfService(service) => vec![
            (keys::SELF_SERVICE_MODE, service.mode.as_str().to_owned()),
            (keys::PUBLIC_PORTAL, flag_text(service.public_portal)),
        ],
        Section::Persona(persona) => vec![
            (keys::PERSONA, persona.active.clone()),
            (
                keys::PROFILE_VISIBILITY,
                persona.profile_visibility.join(","),
            ),
            (
                keys::ROLE_PROFILES,
                serde_json::to_string(&persona.role_profiles)
                    .expect("role profile assignments serialize infallibly"),
            ),
        ],
        Section::Models(models) => {
            let alias = |role: &RoleModel| role.alias.clone().unwrap_or_default();
            vec![
                (keys::EXTRACT_MODEL, alias(&models.extraction)),
                (
                    keys::EXTRACT_REASONING,
                    models.extraction.reasoning.as_str().to_owned(),
                ),
                (keys::CHAT_MODEL, alias(&models.chat)),
                (
                    keys::CHAT_REASONING,
                    models.chat.reasoning.as_str().to_owned(),
                ),
                (keys::REWRITE_MODEL, alias(&models.rewrite)),
                (
                    keys::REWRITE_REASONING,
                    models.rewrite.reasoning.as_str().to_owned(),
                ),
                (
                    keys::MODEL_CONTEXT,
                    serde_json::to_string(&models.context)
                        .expect("context settings serialize infallibly"),
                ),
            ]
        }
        Section::Schedule(schedule) => vec![
            (keys::RESET_WEEKDAY, weekday_text(schedule.reset_weekday)),
            (keys::RESET_TIME, clock_text(schedule.reset_time)),
            (
                keys::ATTENDANCE_MODE,
                attendance_text(schedule.attendance).to_owned(),
            ),
        ],
        Section::Posting(posting) => {
            vec![(
                keys::POST_CHANNEL,
                posting.channel_id.clone().unwrap_or_default(),
            )]
        }
    }
}

/// The section's rows, refused unless each reads back to exactly the value
/// written: nothing the reader would reject (or read differently) is stored.
pub(super) fn encode_checked(section: &Section) -> Result<Rows, SettingsError> {
    let rows = encode(section);
    let mut probe = RuntimeSettings::default();
    for (key, value) in &rows {
        apply(&mut probe, key, value)?;
    }
    let read_back = match section {
        Section::Pings(_) => Section::Pings(probe.pings),
        Section::Watching(_) => Section::Watching(probe.watching),
        Section::Chatbot(_) => Section::Chatbot(probe.chatbot),
        Section::Notifications(_) => Section::Notifications(probe.notifications),
        Section::SelfService(_) => Section::SelfService(probe.self_service),
        Section::Persona(_) => Section::Persona(probe.persona),
        Section::Models(_) => Section::Models(probe.models),
        Section::Schedule(_) => Section::Schedule(probe.schedule),
        Section::Posting(_) => Section::Posting(probe.posting),
    };
    if read_back == *section {
        Ok(rows)
    } else {
        Err(SettingsError::Unrepresentable {
            section: section.name(),
        })
    }
}

impl Section {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Pings(_) => "pings",
            Self::Watching(_) => "watching",
            Self::Chatbot(_) => "chatbot",
            Self::Notifications(_) => "notifications",
            Self::SelfService(_) => "self_service",
            Self::Persona(_) => "persona",
            Self::Models(_) => "models",
            Self::Schedule(_) => "schedule",
            Self::Posting(_) => "posting",
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;

    fn row(edit: impl FnOnce(&mut ContextSettings)) -> String {
        let mut settings = ContextSettings::default();
        edit(&mut settings);
        serde_json::to_string(&settings).unwrap()
    }

    #[test]
    fn stored_reserves_and_windows_must_be_within_the_hard_cap() {
        assert!(context(keys::MODEL_CONTEXT, &row(|_| {})).is_ok());
        for bad in [
            row(|c| c.rewrite.reserve = 0),
            row(|c| c.rewrite.reserve = MAX_CONTEXT_TOKENS + 1),
            row(|c| c.chat.cap = Some(MAX_CONTEXT_TOKENS + 1)),
            row(|c| c.local_default = 0),
        ] {
            assert!(context(keys::MODEL_CONTEXT, &bad).is_err(), "{bad}");
        }
    }
}
