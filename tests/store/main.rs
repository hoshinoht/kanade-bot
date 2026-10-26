//! The SQLite store against the shared conformance suite, plus migrations,
//! ownership, backup/restore and the scheduler's revision retry. Every test
//! works in its own fresh temp directory.

mod backup;
mod chardata;
mod drafts;
mod failures;
mod history;
mod journal;
mod migrate;
mod model_logs;
mod owner;
mod retry;
mod snapshots;
mod support;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::settings::{
    RoleProfileAssignment, RuntimeSettings, Section, SettingsStore, keys, load_settings,
    save_section,
};
use kanade::domain::{
    history::{BlameTarget, ChangeHistory, Origin, changed_fields},
    ids::RandomIds,
    notify::NoticeOutbox,
    schedule::{NewRun, ReminderPolicy, RunSource, RunStatus, ScheduleError, SchedulePolicy},
    scheduler::{Clock, ScheduleStore, SchedulerError, SchedulerService},
};
use kanade::infrastructure::store::{
    SqliteCharData, SqliteStore, attendance_conformance, auth_audit_conformance, card_conformance,
    characters_conformance, chardata_conformance, cherry_pick_conformance, conformance,
    decline_conformance, draft_conformance, guides_conformance, history_conformance,
    journal_conformance, members_conformance, model_log_conformance, outbox_conformance,
    owner_request_conformance, picks_conformance, precondition_conformance, proposal_conformance,
    replay_conformance, run_prompt_conformance, settings_conformance, web_sessions_conformance,
};

#[derive(Clone, Copy)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
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

/// Kuala Lumpur wall clock in September 2026.
fn sep(day: u32, hour: u32) -> DateTime<Utc> {
    Kuala_Lumpur
        .with_ymd_and_hms(2026, 9, day, hour, 0, 0)
        .unwrap()
        .with_timezone(&Utc)
}

/// Creates a standalone HFA run for member 1 in channel 900.
async fn standalone<S: ScheduleStore>(
    service: &mut SchedulerService<S, RandomIds, FixedClock>,
    week_start: DateTime<Utc>,
    datetime: DateTime<Utc>,
    status: RunStatus,
) -> String {
    let run = NewRun {
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start,
        datetime,
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status,
        source: RunSource::Amend,
    };
    let created = service.as_origin(Origin::for_tests()).create_run(run);
    created.await.unwrap()
}

async fn swap_conforms<S>(store: S) -> S
where
    S: ScheduleStore + ChangeHistory + NoticeOutbox,
{
    let policy = swap_policy();
    let (first_at, second_at, week_start) = (sep(29, 20), sep(30, 22), sep(24, 0));
    let mut service = SchedulerService::new(store, RandomIds, FixedClock(sep(28, 12)));
    let planned = RunStatus::Planned;
    let first = standalone(&mut service, week_start, first_at, planned).await;
    let second = standalone(&mut service, week_start, second_at, planned).await;
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
    let record = service.store().load_change(head + 1).await.unwrap();
    let fields = changed_fields(&record.unwrap());
    for run in [&first, &second] {
        assert!(fields.contains(&(BlameTarget::Run(run.clone()), "slot".into())));
    }
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
    let mut policy = swap_policy();
    policy.reset_time = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let week_start = sep(24, 12);
    let mut service = SchedulerService::new(store, RandomIds, FixedClock(sep(24, 13)));
    let s = &mut service;
    let friday_morning = standalone(s, week_start, sep(25, 10), RunStatus::Otot).await;
    let thursday_evening = standalone(s, week_start, sep(24, 20), RunStatus::Planned).await;
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

    let s = &mut service;
    let friday_afternoon = standalone(s, week_start, sep(25, 13), RunStatus::Planned).await;
    let thursday_evening_safe = standalone(s, week_start, sep(24, 20), RunStatus::Planned).await;
    let outcome = service
        .as_origin(Origin::for_tests())
        .swap_run_slots(&friday_afternoon, &thursday_evening_safe, &policy)
        .await
        .unwrap();
    assert_eq!(outcome.value[0].run.datetime, sep(24, 20));
    assert_eq!(outcome.value[1].run.datetime, sep(25, 13));
    service.into_store()
}

#[tokio::test]
async fn slot_swap_conforms() {
    let store = swap_conforms(SqliteStore::open_temp().await).await;
    store.close().await.expect("store closes");
}

#[tokio::test]
async fn slot_swap_respects_non_midnight_reset() {
    let store = swap_respects_non_midnight_reset(SqliteStore::open_temp().await).await;
    store.close().await.expect("store closes");
}

#[tokio::test]
async fn members_conform() {
    members_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn decline_notices_conform() {
    decline_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn settings_conform() {
    settings_conformance::run_suite(SqliteStore::open_temp).await;
}

/// The retired `v5.message_style` row (live stores hold `redesigned`) is
/// neither read nor refused, and saves cannot write it again.
#[tokio::test]
async fn a_retired_message_style_row_is_ignored() {
    let store = SqliteStore::open_temp().await;
    store
        .temp_dir()
        .raw("INSERT INTO config (key, value) VALUES ('v5.message_style', 'redesigned')")
        .await
        .expect("raw row");
    assert!(store.settings_rows().await.unwrap().is_empty());
    let seed = RuntimeSettings::default();
    assert_eq!(load_settings(&store, &seed).await.unwrap(), seed);
    assert!(
        store
            .put_settings_rows(vec![("v5.message_style".into(), "classic".into())])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn replays_conform() {
    replay_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn auth_audit_conforms() {
    auth_audit_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn owner_requests_conform() {
    owner_request_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn run_prompts_conform() {
    run_prompt_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn characters_conform() {
    characters_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn chardata_conforms() {
    chardata_conformance::run_suite(SqliteCharData::open_temp).await;
}

#[tokio::test]
async fn picks_conform() {
    picks_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn guides_conform() {
    guides_conformance::run_suite(SqliteStore::open_temp).await;
}

/// A pick is stored, not recomputed: run and timing picks survive a
/// reopen, and the character's delete after the reopen still drops both.
#[tokio::test]
async fn sqlite_picks_survive_reopen() {
    use kanade::domain::characters::{
        Character, CharacterChange, CharacterRecord, CharacterRole, CharacterStore, PickChange,
        PickScope, PickWrite,
    };
    use kanade::domain::history::{Actor, Surface};

    let dir = support::TempDir::new();
    let config = dir.config("picks-reopen");
    let at = Utc.with_ymd_and_hms(2026, 10, 12, 4, 0, 0).unwrap();
    let record = CharacterRecord {
        at,
        actor: Actor::admin("token"),
        surface: Surface::AdminPortal,
    };
    let store = SqliteStore::open(&config).await.expect("open store");
    let lilac = Character {
        id: "c-1".into(),
        member_id: "1001".into(),
        ign: "Lilac".into(),
        world: "msea".into(),
        class: None,
        level: None,
        role: CharacterRole::Main,
        retired_at: None,
        created_at: at,
        updated_at: at,
        updated_by: Actor::admin("token"),
        version: 1,
    };
    store
        .apply_character(CharacterChange::Create(lilac), record.clone())
        .await
        .expect("create");
    let mut stored = Vec::new();
    for (scope, target) in [(PickScope::Run, "r-1"), (PickScope::Timing, "f-1")] {
        let picked = store
            .apply_pick(
                PickChange::Set {
                    scope,
                    target: target.into(),
                    member_id: "1001".into(),
                    boss: None,
                    character_id: "c-1".into(),
                    version: 0,
                },
                record.clone(),
            )
            .await
            .expect("pick");
        let PickWrite::Picked(pick) = picked else {
            panic!("picked: {picked:?}");
        };
        stored.push((scope, vec![target.to_owned()], pick));
    }
    store.close().await.expect("close store");

    let reopened = SqliteStore::open(&config).await.expect("reopen store");
    for (scope, targets, pick) in &stored {
        assert_eq!(
            reopened.picks(*scope, targets).await.unwrap(),
            std::slice::from_ref(pick)
        );
    }
    reopened
        .apply_character(
            CharacterChange::Delete {
                id: "c-1".into(),
                version: 1,
            },
            record,
        )
        .await
        .expect("delete");
    for (scope, targets, _) in &stored {
        assert!(reopened.picks(*scope, targets).await.unwrap().is_empty());
    }
    reopened.close().await.expect("close reopened store");
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
async fn outbox_conforms() {
    outbox_conformance::run_suite(
        SqliteStore::open_temp,
        async |store: &SqliteStore, source: &str| {
            // A payload version this build does not read.
            let sql = format!(
                "INSERT INTO notice_outbox (source, ordinal, effect_kind, payload, created_at)
                 VALUES ('{source}', 0, 'notice.run.status.cancelled', '{{\"v\":9}}',
                         '2026-09-01T02:00:00+00:00')"
            );
            store.temp_dir().raw(&sql).await.expect("undecodable row");
        },
    )
    .await;
}

#[tokio::test]
async fn web_sessions_conform() {
    web_sessions_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn model_logs_conform() {
    model_log_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn proposal_cards_conform() {
    card_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn proposals_conform() {
    proposal_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn sqlite_store_conforms() {
    conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn journal_conforms() {
    journal_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn drafts_conform() {
    draft_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn attendance_conforms() {
    attendance_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn cherry_picks_conform() {
    cherry_pick_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn preconditions_conform() {
    precondition_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn history_conforms() {
    history_conformance::run_suite(SqliteStore::open_temp).await;
}

#[tokio::test]
async fn write_hints_conform() {
    use kanade::infrastructure::store::hints_conformance::run_suite;
    let store = SqliteStore::open_temp().await;
    run_suite(&store, |store, observer| store.observe_writes(observer)).await;
    store.close().await.expect("store closes");
}
