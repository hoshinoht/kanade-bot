//! Draft storage over SQLite: the merge commit, races, gaps, expiry and
//! the append-only event log. Every test works in its own fresh temp
//! directory.

use chrono::{NaiveTime, TimeZone, Utc, Weekday};
use kanade::domain::drafts::{DraftOp, DraftStatus, DraftStore, LoadedDraft, Target};
use kanade::domain::history::{ChangeHistory, HistoryGap, Origin, Surface};
use kanade::domain::ids::IdGenerator;
use kanade::domain::members::{Directory, Member};
use kanade::domain::notify::DeliveryJournal;
use kanade::domain::schedule::{NewFixedRun, ReminderPolicy, SchedulePolicy};
use kanade::domain::scheduler::{
    DraftError, DraftResult, MergeOutcome, ScheduleStore, SchedulerService, Scope,
};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};

use crate::support::{TempDir, raw, tamper};

#[derive(Default)]
struct CountingIds(u32);

impl IdGenerator for CountingIds {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{:04}", self.0)
    }
}

struct Guild;

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        ["1001", "1002"].contains(&user_id).then(|| Member {
            user_id: user_id.to_owned(),
            display_name: Some(format!("member {user_id}")),
            has_role: true,
            ..Member::default()
        })
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        channel_id == "222"
    }
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::UTC,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60],
        },
        Weekday::Wed,
        NaiveTime::MIN,
    )
}

fn at(day: u32, hour: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
        .single()
        .expect("valid instant")
}

type Service = SchedulerService<SqliteStore, CountingIds, TestClock>;

#[derive(Clone)]
struct TestClock;

impl kanade::domain::scheduler::Clock for TestClock {
    fn now(&self) -> chrono::DateTime<Utc> {
        at(27, 1)
    }
}

async fn open_at(config: &SqliteStoreConfig) -> Service {
    let store = SqliteStore::open(config).await.expect("opens");
    SchedulerService::new(store, CountingIds::default(), TestClock)
}

async fn open() -> (TempDir, Service) {
    let dir = TempDir::new();
    let service = open_at(&dir.config("drafts")).await;
    (dir, service)
}

/// One weekly timing in 222 with runs for 31 Aug, 7 and 14 Sep.
async fn fixture(service: &mut Service) -> (String, Vec<String>) {
    let fixed = service
        .as_origin(Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Mon,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        })
        .await
        .unwrap();
    service
        .as_origin(Origin::for_tests())
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = service.store().load(&Scope::All).await.unwrap();
    let mut runs: Vec<_> = state
        .runs
        .iter()
        .filter(|run| run.fixed_run_id.as_deref() == Some(fixed.as_str()))
        .collect();
    runs.sort_by_key(|run| run.datetime);
    (fixed, runs.iter().map(|run| run.id.clone()).collect())
}

/// A draft moving `run` to Sun 30 Aug 12:00: its id and staged version.
async fn stage_amend(service: &mut Service, run: &str) -> (String, u64) {
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let op = DraftOp::AmendRun {
        run: Target::Existing(run.to_owned()),
        to: at(30, 12),
    };
    let loaded = service
        .add_draft_op("root", &draft.id, draft.version, op, &policy(), &Guild)
        .await
        .unwrap();
    (draft.id, loaded.draft.version)
}

async fn merge(
    service: &mut Service,
    id: &str,
    version: u64,
    request_id: Option<&str>,
) -> DraftResult<MergeOutcome> {
    let (policy, request_id) = (policy(), request_id.map(Into::into));
    service
        .merge_draft("root", id, version, &policy, &Guild, request_id)
        .await
}

async fn loaded(service: &Service, id: &str) -> LoadedDraft {
    service.store().load_draft(id).await.unwrap().unwrap()
}

async fn journal_is_empty(service: &Service) -> bool {
    let view = service.store().load_view().await.unwrap();
    view.targets().is_empty()
}

#[tokio::test]
async fn merge_commits_one_record_and_verifies() {
    let (_dir, mut service) = open().await;
    let (_fixed, runs) = fixture(&mut service).await;
    let seq = service.store().history_head().await.unwrap().seq;
    assert!(journal_is_empty(&service).await);

    let (id, version) = stage_amend(&mut service, &runs[0]).await;
    // Staging wrote draft rows only.
    assert_eq!(service.store().history_head().await.unwrap().seq, seq);
    assert!(journal_is_empty(&service).await);

    let outcome = merge(&mut service, &id, version, None).await.unwrap();
    assert_eq!(outcome.seq, seq + 1);
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.notices[0].channel_id.as_deref(), Some("222"));

    let record = service.store().load_change(outcome.seq).await.unwrap();
    let record = record.unwrap();
    assert_eq!(record.origin.surface, Surface::DraftMerge);
    assert_eq!(
        record.origin.request_id.as_deref(),
        Some(format!("merge:{id}@v2")).as_deref()
    );
    assert_eq!(record.refs, [loaded(&service, &id).await.draft.base]);
    assert!(service.store().verify_history().await.unwrap().is_intact());
    assert!(journal_is_empty(&service).await);
    assert_eq!(
        loaded(&service, &id).await.draft.status,
        DraftStatus::Merged
    );
}

#[tokio::test]
async fn a_sequential_second_merge_is_already_merged() {
    let (_dir, mut service) = open().await;
    let (_fixed, runs) = fixture(&mut service).await;
    let (id, version) = stage_amend(&mut service, &runs[0]).await;
    let first = merge(&mut service, &id, version, Some("merge-a"))
        .await
        .unwrap();
    assert!(matches!(
        merge(&mut service, &id, version + 1, Some("merge-b")).await,
        Err(DraftError::AlreadyMerged { seq }) if seq == first.seq
    ));
    service.into_store().close().await.expect("close");
}

#[tokio::test]
async fn an_exact_retry_after_reopen_is_already_applied() {
    let dir = TempDir::new();
    let config = dir.config("retry");
    let (id, version) = {
        let mut service = open_at(&config).await;
        let (_fixed, runs) = fixture(&mut service).await;
        let (id, version) = stage_amend(&mut service, &runs[0]).await;
        merge(&mut service, &id, version, Some("merge-a"))
            .await
            .unwrap();
        service.into_store().close().await.expect("close");
        (id, version)
    };
    let mut service = open_at(&config).await;
    let seq = service.store().history_head().await.unwrap().seq;
    assert!(matches!(
        merge(&mut service, &id, version, Some("merge-a")).await,
        Err(DraftError::AlreadyApplied { .. })
    ));
    assert_eq!(
        service.store().history_head().await.unwrap().seq,
        seq,
        "nothing was written"
    );
    service.into_store().close().await.expect("close");
}

#[tokio::test]
async fn a_tampered_record_refuses_preview_and_merge() {
    let (dir, mut service) = open().await;
    let (_fixed, runs) = fixture(&mut service).await;
    let (id, version) = stage_amend(&mut service, &runs[0]).await;
    service
        .as_origin(Origin::for_tests())
        .amend_run(&runs[2], at(15, 12), &policy())
        .await
        .unwrap();
    let tip = service.store().history_head().await.unwrap();
    tamper(
        &dir.config("drafts"),
        &format!(
            "DROP TRIGGER change_log_no_update;
         UPDATE change_log SET body = REPLACE(body, '\"HFA\"', '\"HFB\"') WHERE seq = {};",
            tip.seq
        ),
    )
    .await;
    assert!(matches!(
        service.preview_draft(&id, &policy(), &Guild).await,
        Err(DraftError::HistoryGap(_))
    ));
    assert!(matches!(
        merge(&mut service, &id, version, None).await,
        Err(DraftError::HistoryGap(_))
    ));
    // A record that no longer parses is a typed tampered link, not a
    // backend failure.
    tamper(
        &dir.config("drafts"),
        &format!(
            "UPDATE change_log SET body = 'not json' WHERE seq = {};",
            tip.seq
        ),
    )
    .await;
    assert!(matches!(
        service.preview_draft(&id, &policy(), &Guild).await,
        Err(DraftError::HistoryGap(HistoryGap::Tampered { seq })) if seq == tip.seq
    ));
    service.into_store().close().await.expect("close");
}

#[tokio::test]
async fn draft_events_are_append_only() {
    let (dir, mut service) = open().await;
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let config = dir.config("drafts");
    for sql in [
        "UPDATE draft_events SET kind = 'merged'",
        "DELETE FROM draft_events",
    ] {
        let result = raw(&config, sql).await;
        assert!(result.is_err(), "{sql} must be refused");
    }
    let events = service.store().draft_events(&draft.id).await.unwrap();
    assert_eq!(events.len(), 1);
    service.into_store().close().await.expect("close");
}

#[tokio::test]
async fn closed_status_checks_hold_in_the_schema() {
    let (dir, mut service) = open().await;
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let config = dir.config("drafts");
    for sql in [
        // Merged names its record, and only merged does.
        "UPDATE drafts SET status = 'merged', closed_by_kind = 'admin', closed_by_id = 'root'",
        "UPDATE drafts SET merged_seq = 1",
        // Every closed status names who closed it, and only closed ones do.
        "UPDATE drafts SET status = 'discarded'",
        "UPDATE drafts SET status = 'expired'",
        "UPDATE drafts SET closed_by_kind = 'admin', closed_by_id = 'root'",
    ] {
        let result = raw(&config, sql).await;
        assert!(result.is_err(), "{sql} must be refused");
    }
    let loaded = loaded(&service, &draft.id).await;
    assert_eq!(loaded.draft.status, DraftStatus::Open);
    assert_eq!(loaded.draft.closed_by, None);
    service.into_store().close().await.expect("close");
}
