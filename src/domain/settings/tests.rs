use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveTime, Weekday};

use super::codec::{encode_checked, resolve};
use super::*;
use crate::domain::attendance::{AttendanceMode, AttendancePolicy};

fn rows(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn time(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).expect("valid time")
}

#[test]
fn all_keys_are_listed_once() {
    let unique: BTreeSet<_> = keys::ALL.iter().collect();
    assert_eq!(unique.len(), keys::ALL.len());
    let v5: Vec<_> = keys::ALL.iter().filter(|key| key.contains('.')).collect();
    assert!(v5.iter().all(|key| key.starts_with("v5.")), "{v5:?}");
}

#[test]
fn every_section_round_trips_through_its_rows() {
    let mut settings = RuntimeSettings::default();
    settings.pings.countdown_minutes = vec![60, 15];
    settings.watching.channel_ids = vec!["11".into(), "12".into()];
    settings.chatbot.enabled = true;
    settings.models.chat = RoleModel {
        alias: Some("kanata/chat".into()),
        reasoning: Reasoning::High,
    };
    settings.schedule.reset_weekday = Weekday::Wed;
    settings.schedule.attendance = AttendanceMode::V5;
    settings.posting.channel_id = Some("99".into());
    let sections = [
        Section::Pings(settings.pings.clone()),
        Section::Watching(settings.watching.clone()),
        Section::Chatbot(settings.chatbot.clone()),
        Section::Notifications(settings.notifications),
        Section::SelfService(settings.self_service),
        Section::Persona(settings.persona.clone()),
        Section::Models(settings.models.clone()),
        Section::Schedule(settings.schedule),
        Section::Posting(settings.posting.clone()),
    ];
    let mut stored = BTreeMap::new();
    for section in &sections {
        for (key, value) in encode_checked(section).expect("encodes") {
            stored.insert(key.to_owned(), value);
        }
    }
    assert_eq!(
        stored.len(),
        keys::ALL.len(),
        "every key belongs to a section"
    );
    assert_eq!(
        resolve(&stored, &RuntimeSettings::default()).expect("reads"),
        settings
    );
}

#[test]
fn v4_encodings_read_as_v4_did() {
    let settings = resolve(
        &rows(&[
            ("day_of_ping_time", "07:30"),
            ("countdown_minutes", "15;60, 15"),
            ("extract_enabled", "0"),
            ("chat_pilot_rate_window_s", "300.0"),
            ("chat_pilot_global_rate_window_s", "90.5"),
            ("extract_reasoning", "none"),
            ("chat_pilot_think", ""),
            ("extract_model", "  "),
        ]),
        &RuntimeSettings::default(),
    )
    .expect("v4 rows read");
    assert_eq!(settings.pings.day_of_ping_time, time(7, 30));
    assert_eq!(settings.pings.countdown_minutes, [60, 15]);
    assert!(!settings.watching.extract_enabled);
    assert_eq!(settings.chatbot.member_rate.window_s, 300);
    assert_eq!(settings.chatbot.guild_rate.window_s, 91, "rounded up");
    assert_eq!(settings.models.extraction.reasoning, Reasoning::Off);
    assert_eq!(settings.models.chat.reasoning, Reasoning::Inherit);
    assert_eq!(
        settings.models.extraction.alias, None,
        "blank alias is unset"
    );
}

#[test]
fn malformed_values_name_their_key() {
    for (key, value) in [
        ("quiet_mode", "true"),
        ("countdown_minutes", "0"),
        ("chat_pilot_rate_count", "0"),
        ("chat_pilot_rate_window_s", "inf"),
        ("extract_reasoning", ""),
        ("chat_pilot_think", "loud"),
        ("v5.watched_channel_ids", "12,general"),
        ("v5.reset_weekday", "someday"),
        ("v5.attendance_mode", "V5"),
    ] {
        let error =
            resolve(&rows(&[(key, value)]), &RuntimeSettings::default()).expect_err("malformed");
        assert!(
            matches!(&error, SettingsError::Malformed { key: named, .. } if *named == key),
            "{key}: {error:?}"
        );
        assert!(error.to_string().contains(key), "{error}");
    }
}

#[test]
fn unnormalised_sections_are_refused() {
    let pings = Pings {
        day_of_ping_time: time(1, 0),
        countdown_minutes: vec![15, 60],
    };
    assert_eq!(
        encode_checked(&Section::Pings(pings)),
        Err(SettingsError::Unrepresentable { section: "pings" })
    );
    let mut models = Models::default();
    models.extraction.reasoning = Reasoning::Inherit;
    assert!(matches!(
        encode_checked(&Section::Models(models)),
        Err(SettingsError::Malformed {
            key: "extract_reasoning",
            ..
        })
    ));
}

#[test]
fn schedule_policy_comes_from_settings() {
    let mut settings = RuntimeSettings::default();
    settings.pings.day_of_ping_time = time(8, 0);
    settings.schedule.reset_weekday = Weekday::Mon;
    settings.schedule.reset_time = time(2, 0);
    settings.schedule.attendance = AttendanceMode::V5;
    let policy = settings.schedule_policy(chrono_tz::Asia::Kuala_Lumpur);
    assert_eq!(policy.reminders.ping_time, time(8, 0));
    assert_eq!(policy.reminders.countdowns, [60]);
    assert_eq!(policy.reset_weekday, Weekday::Mon);
    assert_eq!(policy.reset_time, time(2, 0));
    assert_eq!(policy.attendance, AttendancePolicy::V5);
    assert_eq!(
        RuntimeSettings::default()
            .schedule_policy(chrono_tz::UTC)
            .attendance,
        AttendancePolicy::V4_COMPAT
    );
}

#[test]
fn self_service_links_need_the_public_portal() {
    let mut service = SelfService {
        mode: SelfServiceMode::LinkFirst,
        public_portal: false,
    };
    assert_eq!(service.effective_mode(), SelfServiceMode::CardsOnly);
    service.public_portal = true;
    assert_eq!(service.effective_mode(), SelfServiceMode::LinkFirst);
}
