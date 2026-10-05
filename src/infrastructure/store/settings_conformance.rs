//! Settings storage every store must keep: stored row over env seed over
//! code default, section writes that touch only their own keys, v4-encoded
//! rows, and malformed rows refused with the key named.

use chrono::{DateTime, NaiveTime, Utc, Weekday};

use crate::domain::attendance::AttendanceMode;
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{
    IdList, Reasoning, RuntimeSettings, Section, SelfServiceMode, SettingsChange,
    SettingsChangeQuery, SettingsError, SettingsStore, diff_rows, keys, load_settings,
    save_section, save_section_recorded,
};

pub async fn run_suite<S: SettingsStore>(make: impl AsyncFn() -> S) {
    unset_keys_fall_back_to_seed_then_default(make().await).await;
    sections_round_trip_and_keep_other_rows(make().await).await;
    profile_visibility_is_private_by_default_and_deduplicated(make().await).await;
    role_profiles_round_trip_in_order(make().await).await;
    v4_rows_read_as_v4_wrote_them(make().await).await;
    malformed_rows_are_errors_naming_the_key(make().await).await;
    refused_writes_store_nothing(make().await).await;
    recorded_saves_append_one_change_with_their_rows(make().await).await;
    refused_recorded_saves_record_nothing(make().await).await;
    rowless_records_leave_the_settings_alone(make().await).await;
}

/// A Limits window clear: a `limits` record with no settings row.
async fn rowless_records_leave_the_settings_alone<S: SettingsStore>(store: S) {
    let at = DateTime::parse_from_rfc3339("2026-09-29T04:00:00Z")
        .expect("instant")
        .with_timezone(&Utc);
    let change = SettingsChange {
        id: 0,
        at,
        actor: Actor::admin("discord:1003"),
        surface: Surface::AdminPortal,
        section: "limits".into(),
        revision: 0,
        values: [(
            "window.1004".to_owned(),
            crate::domain::settings::RowDiff {
                from: r#"{"used":3}"#.into(),
                to: r#"{"used":0}"#.into(),
            },
        )]
        .into(),
    };
    let id = store
        .put_settings_rows_recorded(Vec::new(), change.clone())
        .await
        .expect("recorded");
    assert!(store.settings_rows().await.expect("rows").is_empty());
    let listed = store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .expect("list");
    assert_eq!(listed, vec![SettingsChange { id, ..change }]);
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

async fn role_profiles_round_trip_in_order<S: SettingsStore>(store: S) {
    let defaults = load_settings(&store, &RuntimeSettings::default())
        .await
        .expect("load defaults");
    assert!(defaults.persona.role_profiles.is_empty());

    let value = r#"[{"role_id":"700","profile":"quiet"},{"role_id":"701","profile":"warm"}]"#;
    store
        .put_settings_rows(raw(&[(keys::ROLE_PROFILES, value)]))
        .await
        .expect("put role profiles");
    let loaded = load_settings(&store, &RuntimeSettings::default())
        .await
        .expect("load role profiles");
    assert_eq!(
        loaded.persona.role_profiles,
        [
            crate::domain::settings::RoleProfileAssignment {
                role_id: "700".into(),
                profile: "quiet".into(),
            },
            crate::domain::settings::RoleProfileAssignment {
                role_id: "701".into(),
                profile: "warm".into(),
            },
        ]
    );
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
    wanted.persona.role_profiles = vec![
        crate::domain::settings::RoleProfileAssignment {
            role_id: "700".into(),
            profile: "terse".into(),
        },
        crate::domain::settings::RoleProfileAssignment {
            role_id: "701".into(),
            profile: "calm".into(),
        },
    ];
    wanted.schedule.reset_time = time(3, 30);
    wanted.schedule.attendance = AttendanceMode::V5;
    wanted.posting.channel_id = Some("77".into());
    for section in [
        Section::Chatbot(wanted.chatbot.clone()),
        Section::IdList(IdList::ChatCategories, wanted.chatbot.category_ids.clone()),
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
    assert_eq!(
        rows.get(keys::ROLE_PROFILES).map(String::as_str),
        Some(r#"[{"role_id":"700","profile":"terse"},{"role_id":"701","profile":"calm"}]"#)
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

fn instant(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .expect("instant")
        .with_timezone(&Utc)
}

fn change(
    at: &str,
    actor: Actor,
    before: &RuntimeSettings,
    after: &RuntimeSettings,
) -> SettingsChange {
    SettingsChange {
        id: 0,
        at: instant(at),
        actor,
        surface: Surface::AdminPortal,
        section: "notifications".into(),
        revision: 3,
        values: diff_rows(before, after),
    }
}

async fn recorded_saves_append_one_change_with_their_rows<S: SettingsStore>(store: S) {
    let before = RuntimeSettings::default();
    let mut quiet = before.clone();
    quiet.notifications.quiet_mode = true;
    let first = change(
        "2026-09-29T04:00:00Z",
        Actor::admin("token"),
        &before,
        &quiet,
    );
    save_section_recorded(
        &store,
        &Section::Notifications(quiet.notifications),
        Some(first.clone()),
    )
    .await
    .expect("recorded save");
    assert!(
        load_settings(&store, &before)
            .await
            .expect("load")
            .notifications
            .quiet_mode
    );

    let mut pings = quiet.clone();
    pings.pings.countdown_minutes = vec![30];
    let mut second = change(
        "2026-09-29T05:00:00Z",
        Actor::admin("discord:1001"),
        &quiet,
        &pings,
    );
    second.section = "pings".into();
    second.revision = 4;
    save_section_recorded(
        &store,
        &Section::Pings(pings.pings.clone()),
        Some(second.clone()),
    )
    .await
    .expect("second save");
    // A save without a change (a no-op) appends nothing.
    save_section_recorded(&store, &Section::Pings(pings.pings.clone()), None)
        .await
        .expect("plain save");

    let all = store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .expect("list");
    assert_eq!(all.len(), 2, "newest first, one per recorded save");
    assert_eq!(all[0].section, "pings");
    assert_eq!(all[0].actor, second.actor);
    assert_eq!(all[0].revision, 4);
    assert_eq!(all[0].at, second.at);
    assert_eq!(all[0].values, second.values);
    assert_eq!(all[1].values, first.values);
    assert_eq!(all[1].values[keys::QUIET_MODE].from, "0");
    assert_eq!(all[1].values[keys::QUIET_MODE].to, "1");
    assert!(all[0].id > all[1].id, "ids increase");

    let by_actor = store
        .settings_changes(SettingsChangeQuery {
            actor: Some(Actor::admin("token")),
            ..SettingsChangeQuery::default()
        })
        .await
        .expect("by actor");
    assert_eq!(by_actor.len(), 1);
    assert_eq!(by_actor[0].section, "notifications");
    let window = store
        .settings_changes(SettingsChangeQuery {
            actor: None,
            from: Some(instant("2026-09-29T04:00:00Z")),
            until: Some(instant("2026-09-29T05:00:00Z")),
        })
        .await
        .expect("window");
    assert_eq!(window.len(), 1, "from inclusive, until exclusive");
    assert_eq!(window[0].section, "notifications");
}

async fn refused_recorded_saves_record_nothing<S: SettingsStore>(store: S) {
    let before = RuntimeSettings::default();
    let mut after = before.clone();
    after.chatbot.enabled = true;
    after.chatbot.guild_rate.count = 0;
    let invalid = save_section_recorded(
        &store,
        &Section::Chatbot(after.chatbot.clone()),
        Some(change(
            "2026-09-29T04:00:00Z",
            Actor::admin("token"),
            &before,
            &after,
        )),
    )
    .await;
    assert!(
        matches!(invalid, Err(SettingsError::Malformed { .. })),
        "{invalid:?}"
    );
    let outside = store
        .put_settings_rows_recorded(
            raw(&[("last_digest_week", "x")]),
            change(
                "2026-09-29T04:00:00Z",
                Actor::admin("token"),
                &before,
                &after,
            ),
        )
        .await;
    assert!(
        matches!(outside, Err(StoreError::Constraint(_))),
        "{outside:?}"
    );
    let empty = store
        .put_settings_rows_recorded(
            raw(&[(keys::QUIET_MODE, "1")]),
            change(
                "2026-09-29T04:00:00Z",
                Actor::admin("token"),
                &before,
                &before,
            ),
        )
        .await;
    assert!(matches!(empty, Err(StoreError::Constraint(_))), "{empty:?}");
    assert!(store.settings_rows().await.expect("rows").is_empty());
    assert!(
        store
            .settings_changes(SettingsChangeQuery::default())
            .await
            .expect("list")
            .is_empty()
    );
}
