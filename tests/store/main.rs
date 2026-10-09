//! The SQLite store against the shared conformance suite, plus migrations,
//! ownership, backup/restore and the scheduler's revision retry. Every test
//! works in its own fresh temp directory.

mod backup;
mod drafts;
mod failures;
mod history;
mod journal;
mod migrate;
mod model_logs;
mod owner;
mod retry;
mod support;

use chrono::{NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::settings::{
    RoleProfileAssignment, RuntimeSettings, Section, SettingsStore, keys, load_settings,
    save_section,
};
use kanade::domain::{
    history::{ChangeHistory, Origin, changed_fields},
    ids::RandomIds,
    notify::NoticeOutbox,
    schedule::{NewRun, ReminderPolicy, RunSource, RunStatus, ScheduleError, SchedulePolicy},
    scheduler::{Clock, ScheduleStore, SchedulerError, SchedulerService},
};
use kanade::infrastructure::store::{
    MemoryScheduleStore, SqliteStore, attendance_conformance, card_conformance,
    cherry_pick_conformance, conformance, decline_conformance, draft_conformance,
    history_conformance, journal_conformance, model_log_conformance, precondition_conformance,
    proposal_conformance, web_sessions_conformance,
};

#[derive(Clone, Copy)]
struct FixedClock(chrono::DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        self.0
    }
}

fn swap_policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

async fn swap_conforms<S>(store: S) -> S
where
    S: ScheduleStore + ChangeHistory + NoticeOutbox,
{
    let at = |day, hour| {
        Kuala_Lumpur
            .with_ymd_and_hms(2026, 9, day, hour, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    };
    let policy = swap_policy();
    let first_at = at(29, 20);
    let second_at = at(30, 22);
    let week_start = at(24, 0);
    let mut service = SchedulerService::new(store, RandomIds, FixedClock(at(28, 12)));
    let add = |datetime| NewRun {
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start,
        datetime,
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
    };
    let first = service
        .as_origin(Origin::for_tests())
        .create_run(add(first_at))
        .await
        .unwrap();
    let second = service
        .as_origin(Origin::for_tests())
        .create_run(add(second_at))
        .await
        .unwrap();
    let head = service.store().history_head().await.unwrap().seq;
    let origin = Origin::for_tests().with_request_id("swap-conformance");
    let outcome = service
        .as_origin(origin.clone())
        .swap_run_slots(&first, &second, &policy)
        .await
        .unwrap();
    assert_eq!(outcome.notices.len(), 2, "one move notice per run");
    assert_eq!(outcome.value[0].run.datetime, second_at);
    assert_eq!(outcome.value[1].run.datetime, first_at);
    assert_eq!(service.store().history_head().await.unwrap().seq, head + 1);
    let record = service
        .store()
        .load_change(head + 1)
        .await
        .unwrap()
        .unwrap();
    let fields = changed_fields(&record);
    assert!(fields.contains(&(
        kanade::domain::history::BlameTarget::Run(first.clone()),
        "slot".into()
    )));
    assert!(fields.contains(&(
        kanade::domain::history::BlameTarget::Run(second.clone()),
        "slot".into()
    )));
    assert_eq!(service.store().outbox_notices().await.unwrap().len(), 2);
    assert!(matches!(
        service
            .as_origin(origin)
            .swap_run_slots(&first, &second, &policy)
            .await,
        Err(SchedulerError::AlreadyApplied { .. })
    ));
    assert_eq!(service.store().history_head().await.unwrap().seq, head + 1);
    assert_eq!(service.store().outbox_notices().await.unwrap().len(), 2);
    service.into_store()
}

async fn swap_respects_non_midnight_reset<S>(store: S) -> S
where
    S: ScheduleStore + ChangeHistory + NoticeOutbox,
{
    let at = |day, hour| {
        Kuala_Lumpur
            .with_ymd_and_hms(2026, 9, day, hour, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    };
    let mut policy = swap_policy();
    policy.reset_time = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let week_start = at(24, 12);
    let mut service = SchedulerService::new(store, RandomIds, FixedClock(at(24, 13)));
    let add = |datetime, status| NewRun {
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start,
        datetime,
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status,
        source: RunSource::Amend,
    };
    let friday_morning = service
        .as_origin(Origin::for_tests())
        .create_run(add(at(25, 10), RunStatus::Otot))
        .await
        .unwrap();
    let thursday_evening = service
        .as_origin(Origin::for_tests())
        .create_run(add(at(24, 20), RunStatus::Planned))
        .await
        .unwrap();
    let head = service.store().history_head().await.unwrap().seq;
    assert_eq!(
        service
            .as_origin(Origin::for_tests())
            .swap_run_slots(&friday_morning, &thursday_evening, &policy)
            .await
            .unwrap_err(),
        SchedulerError::Schedule(ScheduleError::SwapLeavesWeek)
    );
    assert_eq!(service.store().history_head().await.unwrap().seq, head);
    assert!(service.store().outbox_notices().await.unwrap().is_empty());

    let friday_afternoon = service
        .as_origin(Origin::for_tests())
        .create_run(add(at(25, 13), RunStatus::Planned))
        .await
        .unwrap();
    let thursday_evening_safe = service
        .as_origin(Origin::for_tests())
        .create_run(add(at(24, 20), RunStatus::Planned))
        .await
        .unwrap();
    let outcome = service
        .as_origin(Origin::for_tests())
        .swap_run_slots(&friday_afternoon, &thursday_evening_safe, &policy)
        .await
        .unwrap();
    assert_eq!(outcome.value[0].run.datetime, at(24, 20));
    assert_eq!(outcome.value[1].run.datetime, at(25, 13));
    service.into_store()
}

#[tokio::test]
async fn memory_slot_swap_conforms() {
    let _ = swap_conforms(MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_slot_swap_conforms() {
    let dir = support::TempDir::new();
    let store = swap_conforms(
        SqliteStore::open(&dir.config("slot-swap"))
            .await
            .expect("fresh store opens"),
    )
    .await;
    store.close().await.expect("store closes");
}

#[tokio::test]
async fn memory_slot_swap_respects_non_midnight_reset() {
    let _ = swap_respects_non_midnight_reset(MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_slot_swap_respects_non_midnight_reset() {
    let dir = support::TempDir::new();
    let store = swap_respects_non_midnight_reset(
        SqliteStore::open(&dir.config("slot-swap-non-midnight"))
            .await
            .expect("fresh store opens"),
    )
    .await;
    store.close().await.expect("store closes");
}

#[tokio::test]
async fn memory_members_conform() {
    kanade::infrastructure::store::members_conformance::run_suite(async || {
        MemoryScheduleStore::new()
    })
    .await;
}

#[tokio::test]
async fn memory_decline_notices_conform() {
    decline_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_decline_notices_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    decline_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("declines-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn sqlite_members_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    kanade::infrastructure::store::members_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("members-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_settings_conform() {
    kanade::infrastructure::store::settings_conformance::run_suite(async || {
        MemoryScheduleStore::new()
    })
    .await;
}

#[tokio::test]
async fn sqlite_settings_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    kanade::infrastructure::store::settings_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("settings-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_replays_conform() {
    kanade::infrastructure::store::replay_conformance::run_suite(async || {
        MemoryScheduleStore::new()
    })
    .await;
}

#[tokio::test]
async fn sqlite_replays_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    kanade::infrastructure::store::replay_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("replays-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn sqlite_profile_visibility_defaults_private_and_survives_reopen() {
    let dir = support::TempDir::new();
    let config = dir.config("profile-visibility");
    let store = SqliteStore::open(&config).await.expect("fresh store opens");
    assert!(
        load_settings(&store, &RuntimeSettings::default())
            .await
            .expect("default settings")
            .persona
            .profile_visibility
            .is_empty()
    );
    store
        .put_settings_rows(vec![(
            keys::PROFILE_VISIBILITY.to_owned(),
            "bold,calm,bold".to_owned(),
        )])
        .await
        .expect("save visibility");
    store.close().await.expect("close store");

    let reopened = SqliteStore::open(&config).await.expect("reopen store");
    let settings = load_settings(&reopened, &RuntimeSettings::default())
        .await
        .expect("reopened settings");
    assert_eq!(settings.persona.profile_visibility, ["bold", "calm"]);
    reopened.close().await.expect("close reopened store");
}

#[tokio::test]
async fn sqlite_role_profiles_preserve_order_across_reopen() {
    let dir = support::TempDir::new();
    let config = dir.config("role-profiles");
    let store = SqliteStore::open(&config).await.expect("fresh store opens");
    let mut persona = RuntimeSettings::default().persona;
    persona.role_profiles = vec![
        RoleProfileAssignment {
            role_id: "700".into(),
            profile: "quiet".into(),
        },
        RoleProfileAssignment {
            role_id: "701".into(),
            profile: "warm".into(),
        },
    ];
    save_section(&store, &Section::Persona(persona.clone()))
        .await
        .expect("save assignments");
    let rows = store.settings_rows().await.expect("stored rows");
    assert_eq!(
        rows.get(keys::ROLE_PROFILES).map(String::as_str),
        Some(r#"[{"role_id":"700","profile":"quiet"},{"role_id":"701","profile":"warm"}]"#)
    );
    store.close().await.expect("close store");

    let reopened = SqliteStore::open(&config).await.expect("reopen store");
    let settings = load_settings(&reopened, &RuntimeSettings::default())
        .await
        .expect("reopened settings");
    assert_eq!(settings.persona.role_profiles, persona.role_profiles);
    reopened.close().await.expect("close reopened store");
}

#[tokio::test]
async fn memory_outbox_conforms() {
    kanade::infrastructure::store::outbox_conformance::run_suite(async || {
        MemoryScheduleStore::new()
    })
    .await;
}

#[tokio::test]
async fn sqlite_outbox_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    kanade::infrastructure::store::outbox_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("outbox-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_web_sessions_conform() {
    web_sessions_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_web_sessions_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    web_sessions_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("sessions-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_model_logs_conform() {
    model_log_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_model_logs_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    model_log_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("logs-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_proposal_cards_conform() {
    card_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_proposal_cards_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    card_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("cards-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_proposals_conform() {
    proposal_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_proposals_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    proposal_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("proposals-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn sqlite_store_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("conform-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_journal_conforms() {
    journal_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_journal_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    journal_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("journal-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_drafts_conform() {
    draft_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_drafts_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    draft_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("drafts-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_attendance_conforms() {
    attendance_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_attendance_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    attendance_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("attendance-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_cherry_picks_conform() {
    cherry_pick_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_cherry_picks_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    cherry_pick_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("picks-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_preconditions_conform() {
    precondition_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_preconditions_conform() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    precondition_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("preconditions-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_history_conforms() {
    history_conformance::run_suite(async || MemoryScheduleStore::new()).await;
}

#[tokio::test]
async fn sqlite_history_conforms() {
    let dir = support::TempDir::new();
    let counter = std::sync::atomic::AtomicUsize::new(0);
    history_conformance::run_suite(async || {
        let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        SqliteStore::open(&dir.config(&format!("history-{n}")))
            .await
            .expect("fresh store opens")
    })
    .await;
}

#[tokio::test]
async fn memory_write_hints_conform() {
    kanade::infrastructure::store::hints_conformance::run_suite(
        &MemoryScheduleStore::new(),
        |store, observer| store.observe_writes(observer),
    )
    .await;
}

#[tokio::test]
async fn sqlite_write_hints_conform() {
    let dir = support::TempDir::new();
    let store = SqliteStore::open(&dir.config("hints"))
        .await
        .expect("fresh store opens");
    kanade::infrastructure::store::hints_conformance::run_suite(&store, |store, observer| {
        store.observe_writes(observer)
    })
    .await;
    store.close().await.expect("store closes");
}
