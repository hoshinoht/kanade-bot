//! Draft storage over SQLite: history gaps under the shared merge, the
//! append-only event log and the closed-status checks. Every test works in
//! its own fresh temp directory.

use chrono::{NaiveTime, TimeZone, Utc, Weekday};
use kanade_domain::drafts::{
    DraftCreated, DraftKind, DraftStatus, DraftStore, LoadedDraft, NewDraft,
};
use kanade_domain::history::{Actor, ChangeHistory, HistoryGap, Origin};
use kanade_domain::ids::IdGenerator;
use kanade_domain::members::{Directory, Member};
use kanade_domain::requests::{NoFreezes, RequestSpec, Subject};
use kanade_domain::schedule::{NewFixedRun, ReminderPolicy, SchedulePolicy};
use kanade_domain::scheduler::{DraftError, RequestError, ScheduleStore, SchedulerService, Scope};
use kanade_store::{SqliteStore, SqliteStoreConfig};

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
        ["1001", "1002", "1003"].contains(&user_id).then(|| Member {
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

impl kanade_domain::scheduler::Clock for TestClock {
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

async fn loaded(service: &Service, id: &str) -> LoadedDraft {
    service.store().load_draft(id).await.unwrap().unwrap()
}

/// A tampered record after a request's base refuses its approval: the
/// shared draft merge reads the chain as a gap.
#[tokio::test]
async fn a_tampered_record_refuses_a_request_merge() {
    let (dir, mut service) = open().await;
    let (_fixed, runs) = fixture(&mut service).await;
    let join = RequestSpec::Join(Subject::Run(runs[0].clone()));
    let request = service
        .submit_request("1003", "please", join, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
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
    let approve = async |service: &mut Service| {
        let admin = Actor::admin("root");
        service
            .approve_request(&admin, &request.id, 1, None, &policy(), &Guild, &NoFreezes)
            .await
    };
    assert!(matches!(
        approve(&mut service).await,
        Err(RequestError::Draft(DraftError::HistoryGap(_)))
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
        approve(&mut service).await,
        Err(RequestError::Draft(DraftError::HistoryGap(HistoryGap::Tampered { seq }))) if seq == tip.seq
    ));
    service.into_store().close().await.expect("close");
}

/// An open administrator draft, as older stores hold them.
async fn admin_draft(service: &Service) -> String {
    let (snapshot, base) = service.store().snapshot_with_head().await.unwrap();
    let new = NewDraft {
        id: "draft-1".into(),
        kind: DraftKind::Admin,
        title: "retime".into(),
        author: Actor::admin("root"),
        base,
        base_revision: snapshot.revision,
        request_type: None,
        subject: None,
        at: at(27, 1),
        request: None,
        submit: None,
    };
    match service.store().create_draft(new).await.unwrap() {
        DraftCreated::Created(draft) => draft.id,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn draft_events_are_append_only() {
    let (dir, service) = open().await;
    let draft = admin_draft(&service).await;
    let config = dir.config("drafts");
    for sql in [
        "UPDATE draft_events SET kind = 'merged'",
        "DELETE FROM draft_events",
    ] {
        let result = raw(&config, sql).await;
        assert!(result.is_err(), "{sql} must be refused");
    }
    let events = service.store().draft_events(&draft).await.unwrap();
    assert_eq!(events.len(), 1);
    service.into_store().close().await.expect("close");
}

#[tokio::test]
async fn closed_status_checks_hold_in_the_schema() {
    let (dir, service) = open().await;
    let draft = admin_draft(&service).await;
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
    let loaded = loaded(&service, &draft).await;
    assert_eq!(loaded.draft.status, DraftStatus::Open);
    assert_eq!(loaded.draft.closed_by, None);
    service.into_store().close().await.expect("close");
}
