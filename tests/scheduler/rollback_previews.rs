//! Rollback previews (`preview_revert_changes`, `preview_restore_week`,
//! `preview_revert_by_actor`): they write nothing, and the rows they plan are
//! the rows the apply records.

use std::collections::BTreeSet;

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::history::{
    Actor, ChangeHistory, Origin, RevertMode, RevertOutcome, RowChange, RowKey, Surface,
};
use kanade::domain::ids::IdGenerator;
use kanade::domain::schedule::{
    NewFixedRun, ReminderPolicy, RsvpSource, RsvpState, SchedulePolicy, ScheduleSnapshot,
};
use kanade::domain::scheduler::{ScheduleStore, SchedulerService, Scope};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::TestClock;

#[derive(Default)]
struct CountingIds(u32);

impl IdGenerator for CountingIds {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{:04}", self.0)
    }
}

type Service = SchedulerService<MemoryScheduleStore, CountingIds, TestClock>;

fn kl(month: u32, day: u32, hour: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, day, hour, 0, 0)
        .unwrap()
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

fn admin() -> Origin {
    Origin::new(Actor::admin("root"), Surface::AdminPortal)
}

fn member(id: &str) -> Origin {
    Origin::new(Actor::member(id), Surface::Discord)
}

struct Fixture {
    service: Service,
    /// This week's run (Sat 29 Aug 20:00 KL) and its boss week.
    run: String,
    week: DateTime<Utc>,
}

async fn fixture() -> Fixture {
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        CountingIds::default(),
        TestClock::new(kl(8, 27, 1)),
    );
    service
        .as_origin(admin())
        .add_fixed_run(NewFixedRun {
            owner_pinned: false,
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        })
        .await
        .unwrap();
    service
        .as_origin(admin())
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let at = kl(8, 29, 20).with_timezone(&Utc);
    let run = snapshot(&service)
        .await
        .runs
        .into_iter()
        .find(|run| run.datetime == at)
        .unwrap();
    Fixture {
        service,
        run: run.id,
        week: run.week_start,
    }
}

async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

async fn answer(f: &mut Fixture, user: &str, state: RsvpState) -> u64 {
    f.service
        .as_origin(member(user))
        .set_rsvp(&f.run, user, state, RsvpSource::Chat)
        .await
        .unwrap();
    f.service.store().history_head().await.unwrap().seq
}

/// Rows and seq of a `Reverted` outcome.
fn reverted(outcome: RevertOutcome) -> (Vec<RowChange>, Option<u64>) {
    match outcome {
        RevertOutcome::Reverted { rows, seq, .. } => (rows, seq),
        other => panic!("not reverted: {other:?}"),
    }
}

/// Previewing changes nothing (state, revision, history head), the preview
/// equals the apply, and the applied rows are the record's.
async fn check(
    f: &mut Fixture,
    preview: RevertOutcome,
    apply: impl AsyncFnOnce(&mut Service) -> RevertOutcome,
) {
    let before = snapshot(&f.service).await;
    let head = f.service.store().history_head().await.unwrap();
    let (planned, none) = reverted(preview);
    assert_eq!(none, None, "a preview commits nothing");
    assert!(!planned.is_empty());
    assert_eq!(snapshot(&f.service).await, before, "preview wrote nothing");
    assert_eq!(f.service.store().history_head().await.unwrap(), head);

    let (rows, seq) = reverted(apply(&mut f.service).await);
    assert_eq!(rows, planned, "the preview is what the apply does");
    let seq = seq.expect("applied");
    assert_eq!(seq, head.seq + 1);
    let record = f.service.store().load_change(seq).await.unwrap().unwrap();
    assert_eq!(record.rows, rows, "rows are the recorded rows");
    assert_eq!(record.origin.surface, Surface::Rollback);
}

#[tokio::test]
async fn previewing_a_revert_writes_nothing_and_matches_the_apply() {
    let mut f = fixture().await;
    let seq = answer(&mut f, "1002", RsvpState::No).await;
    let reminders = policy().reminders;
    let none = BTreeSet::new();
    let preview = f
        .service
        .preview_revert_changes(&[seq], RevertMode::Strict, &reminders, &none)
        .await
        .unwrap();
    check(&mut f, preview, async |service| {
        service
            .revert_changes("root", None, &[seq], RevertMode::Strict, &reminders, &none)
            .await
            .unwrap()
    })
    .await;
    // The answer (and its at-risk recount) is gone again.
    assert!(
        !snapshot(&f.service)
            .await
            .rsvps
            .iter()
            .any(|rsvp| rsvp.user_id == "1002")
    );
}

#[tokio::test]
async fn previewing_a_week_restore_and_an_actor_revert_match_the_apply() {
    let mut f = fixture().await;
    let mark = snapshot(&f.service).await.revision;
    answer(&mut f, "1001", RsvpState::Yes).await;
    answer(&mut f, "1002", RsvpState::Maybe).await;
    let (week, reminders, none) = (f.week, policy().reminders, BTreeSet::new());
    let preview = f
        .service
        .preview_restore_week(week, mark, RevertMode::Strict, &reminders, &none)
        .await
        .unwrap();
    check(&mut f, preview, async |service| {
        service
            .restore_week_to(
                "root",
                None,
                week,
                mark,
                RevertMode::Strict,
                &reminders,
                &none,
            )
            .await
            .unwrap()
    })
    .await;

    answer(&mut f, "1002", RsvpState::No).await;
    let spammer = Actor::member("1002");
    let since = DateTime::<Utc>::UNIX_EPOCH;
    let preview = f
        .service
        .preview_revert_by_actor(&spammer, since, RevertMode::Force, &reminders, &none)
        .await
        .unwrap();
    check(&mut f, preview, async |service| {
        service
            .revert_by_actor(
                "root",
                None,
                &spammer,
                since,
                RevertMode::Force,
                &reminders,
                &none,
            )
            .await
            .unwrap()
    })
    .await;
}

#[tokio::test]
async fn a_strict_preview_reports_conflicts_without_writing() {
    let mut f = fixture().await;
    let seq = answer(&mut f, "1002", RsvpState::No).await;
    answer(&mut f, "1002", RsvpState::Yes).await;
    let head = f.service.store().history_head().await.unwrap();
    let (reminders, none) = (policy().reminders, BTreeSet::new());
    let preview = f
        .service
        .preview_revert_changes(&[seq], RevertMode::Strict, &reminders, &none)
        .await
        .unwrap();
    let RevertOutcome::Conflicts { seqs, conflicts } = preview else {
        panic!("{preview:?}");
    };
    assert_eq!(seqs, [seq]);
    assert!(conflicts.iter().any(|conflict| matches!(
        &conflict.key,
        RowKey::Rsvp { user_id, .. } if user_id == "1002"
    )));
    assert_eq!(f.service.store().history_head().await.unwrap(), head);
}
