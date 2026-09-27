//! Settings storage every store must keep: stored row over env seed over
//! code default, section writes that touch only their own keys, v4-encoded
//! rows, and malformed rows refused with the key named.

use chrono::{NaiveTime, Weekday};

use crate::domain::attendance::AttendanceMode;
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{
    Reasoning, RuntimeSettings, Section, SelfServiceMode, SettingsError, SettingsStore, keys,
    load_settings, save_section,
};

pub async fn run_suite<S: SettingsStore>(make: impl AsyncFn() -> S) {
    unset_keys_fall_back_to_seed_then_default(make().await).await;
    sections_round_trip_and_keep_other_rows(make().await).await;
    profile_visibility_is_private_by_default_and_deduplicated(make().await).await;
    v4_rows_read_as_v4_wrote_them(make().await).await;
    malformed_rows_are_errors_naming_the_key(make().await).await;
    refused_writes_store_nothing(make().await).await;
}

fn time(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).expect("valid time")
}

fn raw(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn seed() -> RuntimeSettings {
    let mut seed = RuntimeSettings::default();
    seed.watching.extract_enabled = false;
    seed.chatbot.enabled = true;
    seed.models.extraction.alias = Some("env/extract".into());
    seed.schedule.reset_weekday = Weekday::Wed;
    seed
}

async fn unset_keys_fall_back_to_seed_then_default<S: SettingsStore>(store: S) {
    assert_eq!(
        load_settings(&store, &RuntimeSettings::default())
            .await
            .expect("load"),
        RuntimeSettings::default(),
        "an empty table is the code default"
    );
    assert_eq!(load_settings(&store, &seed()).await.expect("load"), seed());

    store
        .put_settings_rows(raw(&[
            (keys::EXTRACT_ENABLED, "1"),
            (keys::EXTRACT_MODEL, "db/extract"),
            (keys::QUIET_MODE, "1"),
        ]))
        .await
        .expect("put");
    let loaded = load_settings(&store, &seed()).await.expect("load");
    assert!(loaded.watching.extract_enabled, "row over seed");
    assert_eq!(
        loaded.models.extraction.alias.as_deref(),
        Some("db/extract")
    );
    assert!(loaded.notifications.quiet_mode, "row over default");
    assert!(loaded.chatbot.enabled, "seed without a row");
    assert_eq!(loaded.schedule.reset_weekday, Weekday::Wed);
    assert_eq!(
        loaded.pings,
        RuntimeSettings::default().pings,
        "default without seed or row"
    );
}

async fn profile_visibility_is_private_by_default_and_deduplicated<S: SettingsStore>(store: S) {
    let defaults = load_settings(&store, &RuntimeSettings::default())
        .await
        .expect("load defaults");
    assert!(defaults.persona.profile_visibility.is_empty());

    store
        .put_settings_rows(raw(&[(keys::PROFILE_VISIBILITY, "calm,terse,calm")]))
        .await
        .expect("put visibility");
    let loaded = load_settings(&store, &RuntimeSettings::default())
        .await
        .expect("load visibility");
    assert_eq!(loaded.persona.profile_visibility, ["calm", "terse"]);
}

async fn sections_round_trip_and_keep_other_rows<S: SettingsStore>(store: S) {
    let mut wanted = seed();
    wanted.notifications.quiet_mode = true;
    save_section(&store, &Section::Notifications(wanted.notifications))
        .await
        .expect("save notifications");

    wanted.chatbot.category_ids = vec!["1001".into(), "1002".into()];
    wanted.chatbot.member_rate.count = 2;
    wanted.chatbot.guild_rate.window_s = 60;
    wanted.models.chat.alias = Some("kanata/chat".into());
    wanted.models.chat.reasoning = Reasoning::Medium;
    wanted.models.extraction.alias = None;
    wanted.self_service.mode = SelfServiceMode::LinkFirst;
    wanted.self_service.public_portal = true;
    wanted.persona.profile_visibility = vec!["terse".into(), "calm".into()];
    wanted.schedule.reset_time = time(3, 30);
    wanted.schedule.attendance = AttendanceMode::V5;
    wanted.posting.channel_id = Some("77".into());
    for section in [
        Section::Chatbot(wanted.chatbot.clone()),
        Section::Models(wanted.models.clone()),
        Section::SelfService(wanted.self_service),
        Section::Persona(wanted.persona.clone()),
        Section::Schedule(wanted.schedule),
        Section::Posting(wanted.posting.clone()),
    ] {
        save_section(&store, &section).await.expect("save");
    }
    // An unset alias is stored blank, so the seed's alias applies again.
    wanted.models.extraction.alias = Some("env/extract".into());
    assert_eq!(load_settings(&store, &seed()).await.expect("load"), wanted);

    let rows = store.settings_rows().await.expect("rows");
    assert_eq!(rows.get(keys::QUIET_MODE).map(String::as_str), Some("1"));
    assert_eq!(
        rows.get(keys::CHAT_CATEGORIES).map(String::as_str),
        Some("1001,1002")
    );
    assert_eq!(
        rows.get(keys::RESET_TIME).map(String::as_str),
        Some("03:30")
    );
    assert_eq!(
        rows.get(keys::PROFILE_VISIBILITY).map(String::as_str),
        Some("terse,calm")
    );
    assert!(
        !rows.contains_key(keys::DAY_OF_PING_TIME),
        "unsaved sections write nothing"
    );
}

async fn v4_rows_read_as_v4_wrote_them<S: SettingsStore>(store: S) {
    store
        .put_settings_rows(raw(&[
            (keys::DAY_OF_PING_TIME, "09:15"),
            (keys::COUNTDOWN_MINUTES, "60,15"),
            (keys::PAUSED, "1"),
            (keys::CHAT_MODE, "0"),
            (keys::PERSONA, "kanade"),
            (keys::CHAT_RATE_COUNT, "6"),
            (keys::CHAT_RATE_WINDOW, "120.0"),
            (keys::EXTRACT_REASONING, "low"),
            (keys::CHAT_REASONING, ""),
            (keys::CHAT_MODEL, "kanata/chat"),
        ]))
        .await
        .expect("put");
    let loaded = load_settings(&store, &seed()).await.expect("load");
    assert_eq!(loaded.pings.day_of_ping_time, time(9, 15));
    assert_eq!(loaded.pings.countdown_minutes, [60, 15]);
    assert!(loaded.watching.paused);
    assert!(!loaded.chatbot.enabled, "a stored 0 beats the seed");
    assert_eq!(loaded.persona.active, "kanade");
    assert_eq!(loaded.chatbot.member_rate.count, 6);
    assert_eq!(loaded.chatbot.member_rate.window_s, 120);
    assert_eq!(loaded.models.extraction.reasoning, Reasoning::Low);
    assert_eq!(loaded.models.chat.reasoning, Reasoning::Inherit);
    assert_eq!(loaded.models.chat.alias.as_deref(), Some("kanata/chat"));
}

async fn malformed_rows_are_errors_naming_the_key<S: SettingsStore>(store: S) {
    store
        .put_settings_rows(raw(&[(keys::CHAT_RATE_COUNT, "lots")]))
        .await
        .expect("raw rows are not validated");
    let error = load_settings(&store, &seed())
        .await
        .expect_err("malformed row");
    assert_eq!(
        error,
        SettingsError::Malformed {
            key: keys::CHAT_RATE_COUNT,
            value: "lots".into(),
            reason: "expected a whole number of at least 0".into(),
        }
    );
    assert!(
        error.to_string().contains("chat_pilot_rate_count"),
        "{error}"
    );
}

async fn refused_writes_store_nothing<S: SettingsStore>(store: S) {
    let outside = store
        .put_settings_rows(raw(&[
            (keys::QUIET_MODE, "1"),
            ("last_digest_week", "2026-09-24T00:00:00+00:00"),
        ]))
        .await;
    assert!(
        matches!(outside, Err(StoreError::Constraint(_))),
        "{outside:?}"
    );

    let mut chatbot = RuntimeSettings::default().chatbot;
    chatbot.enabled = true;
    chatbot.guild_rate.count = 0;
    let invalid = save_section(&store, &Section::Chatbot(chatbot)).await;
    assert!(
        matches!(
            invalid,
            Err(SettingsError::Malformed {
                key: keys::CHAT_GLOBAL_RATE_COUNT,
                ..
            })
        ),
        "{invalid:?}"
    );
    assert!(
        store.settings_rows().await.expect("rows").is_empty(),
        "no partial write"
    );
}
