//! The behaviour every [`ScheduleStore`] must show; a durable store runs the
//! same [`run_suite`] as the in-memory one. Failures panic with the check name.
//!
//! Checks that need delivery-journal state (an unproven-retired reminder
//! never reopens) are in [`super::journal_conformance`].

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};

use crate::domain::history::{Actor, ChangeMeta, Origin, Surface};
use crate::domain::schedule::{
    Change, ChangeSet, FixedRun, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
    ScheduleSnapshot,
};
use crate::domain::scheduler::{ScheduleStore, Scope, StoreError};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S: ScheduleStore>(make: impl AsyncFn() -> S) {
    empty_store_loads_nothing(&make().await).await;
    commits_round_trip_in_contract_order(&make().await).await;
    scopes_select_runs_with_their_rows(&make().await).await;
    puts_replace_by_key_and_deletes_remove(&make().await).await;
    duplicate_reminder_kind_is_rejected_atomically(&make().await).await;
    duplicate_weekly_week_is_rejected_atomically(&make().await).await;
    rebuilt_kind_in_one_commit_is_accepted(&make().await).await;
    cross_week_swap_commits(&make().await).await;
    stale_revision_commit_conflicts(&make().await).await;
    rows_without_a_run_are_rejected_atomically(&make().await).await;
    instants_keep_microsecond_precision(&make().await).await;
}

fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 31, hour, minute, 0)
        .single()
        .expect("valid instant")
}

pub(crate) fn week() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 26, 16, 0, 0)
        .single()
        .expect("valid instant")
}

fn fixed(id: &str, weekday: Weekday, hour: u32) -> FixedRun {
    FixedRun {
        id: id.into(),
        owner_id: "42".into(),
        channel_id: Some("900".into()),
        bosses: vec!["HFA".into()],
        weekday,
        time: NaiveTime::from_hms_opt(hour, 30, 0).expect("valid time"),
        participants: vec!["1".into(), "2".into()],
        note: None,
        attendance_default: Default::default(),
        standing: Vec::new(),
    }
}

pub(crate) fn run(
    id: &str,
    fixed_run_id: Option<&str>,
    week_start: DateTime<Utc>,
    hour: u32,
) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: fixed_run_id.map(Into::into),
        channel_id: Some("900".into()),
        week_start,
        datetime: at(hour, 30),
        bosses: vec!["HFA".into()],
        participants: vec!["1".into(), "2".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn reminder(id: &str, run_id: &str, kind: &str, hour: u32) -> Reminder {
    Reminder {
        id: id.into(),
        run_id: run_id.into(),
        kind: kind.into(),
        fire_at: at(hour, 0),
        sent_at: None,
        message_id: None,
    }
}

fn rsvp(run_id: &str, user_id: &str, state: RsvpState) -> Rsvp {
    Rsvp {
        run_id: run_id.into(),
        user_id: user_id.into(),
        state,
        source: RsvpSource::Chat,
        at: at(1, 0),
    }
}

/// The attribution conformance checks commit with.
pub fn meta() -> ChangeMeta {
    ChangeMeta {
        origin: Origin::new(Actor::admin("conformance"), Surface::Import),
        at: Utc
            .with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
            .single()
            .expect("valid instant"),
        notices: Vec::new(),
        refs: Vec::new(),
        request_digest: None,
        expect: Default::default(),
        outbox: Vec::new(),
    }
}

/// Commit at the store's current revision.
async fn commit<S: ScheduleStore>(store: &S, changes: Vec<Change>) -> Result<(), StoreError> {
    let revision = load(store, Scope::Weeks(Vec::new())).await.revision;
    store
        .commit(revision, ChangeSet { changes }, meta())
        .await
        .map(|_| ())
}

async fn load<S: ScheduleStore>(store: &S, scope: Scope) -> ScheduleSnapshot {
    store.load(&scope).await.expect("load succeeds")
}

fn ids<T>(rows: &[T], id: impl Fn(&T) -> &str) -> Vec<String> {
    rows.iter().map(|row| id(row).to_owned()).collect()
}

async fn seed<S: ScheduleStore>(store: &S) {
    let next_week = week() + chrono::TimeDelta::days(7);
    commit(
        store,
        vec![
            Change::PutFixedRun(fixed("f-b", Weekday::Tue, 9)),
            Change::PutFixedRun(fixed("f-a", Weekday::Mon, 21)),
            Change::PutRun(run("r-late", None, week(), 20)),
            Change::PutRun(run("r-early", Some("f-a"), week(), 10)),
            Change::PutRun(run("r-next", Some("f-a"), next_week, 12)),
            Change::PutReminder(reminder("m-2", "r-early", "day_of", 9)),
            Change::PutReminder(reminder("m-1", "r-early", "countdown_60", 9)),
            Change::PutReminder(reminder("m-3", "r-late", "day_of", 8)),
            Change::PutReminder(reminder("m-4", "r-next", "day_of", 8)),
            Change::PutRsvp(rsvp("r-early", "2", RsvpState::No)),
            Change::PutRsvp(rsvp("r-early", "1", RsvpState::Yes)),
            Change::PutRsvp(rsvp("r-next", "1", RsvpState::Yes)),
        ],
    )
    .await
    .expect("seed commit succeeds");
}

async fn empty_store_loads_nothing<S: ScheduleStore>(store: &S) {
    assert_eq!(
        load(store, Scope::All).await,
        ScheduleSnapshot::default(),
        "empty_store_loads_nothing"
    );
    commit(store, Vec::new())
        .await
        .expect("an empty change set commits");
    assert_eq!(
        load(store, Scope::All).await.revision,
        0,
        "an empty commit keeps the revision"
    );
}

async fn commits_round_trip_in_contract_order<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let all = load(store, Scope::All).await;
    assert_eq!(ids(&all.fixed_runs, |row| &row.id), ["f-a", "f-b"]);
    assert_eq!(
        ids(&all.runs, |row| &row.id),
        ["r-early", "r-next", "r-late"]
    );
    assert_eq!(
        ids(&all.reminders, |row| &row.id),
        ["m-1", "m-2", "m-3", "m-4"],
        "reminders order by (run_id, fire_at, kind)"
    );
    let users: Vec<(&str, &str)> = all
        .rsvps
        .iter()
        .map(|row| (row.run_id.as_str(), row.user_id.as_str()))
        .collect();
    assert_eq!(users, [("r-early", "1"), ("r-early", "2"), ("r-next", "1")]);
    assert_eq!(all.runs[0], run("r-early", Some("f-a"), week(), 10));
}

async fn scopes_select_runs_with_their_rows<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let weekly = load(store, Scope::Weeks(vec![week()])).await;
    assert_eq!(ids(&weekly.fixed_runs, |row| &row.id), ["f-a", "f-b"]);
    assert_eq!(ids(&weekly.runs, |row| &row.id), ["r-early", "r-late"]);
    assert_eq!(ids(&weekly.reminders, |row| &row.id), ["m-1", "m-2", "m-3"]);
    assert_eq!(weekly.rsvps.len(), 2);

    let none = load(store, Scope::Weeks(Vec::new())).await;
    assert_eq!(none.fixed_runs.len(), 2, "timings are always loaded");
    assert!(none.runs.is_empty() && none.reminders.is_empty() && none.rsvps.is_empty());

    let one = load(store, Scope::Run("r-next".into())).await;
    assert_eq!(ids(&one.runs, |row| &row.id), ["r-next"]);
    assert_eq!(ids(&one.reminders, |row| &row.id), ["m-4"]);
    assert_eq!(one.rsvps.len(), 1);

    let owner = load(store, Scope::Reminder("m-2".into())).await;
    assert_eq!(ids(&owner.runs, |row| &row.id), ["r-early"]);
    assert_eq!(ids(&owner.reminders, |row| &row.id), ["m-1", "m-2"]);

    let missing = load(store, Scope::Run("absent".into())).await;
    assert!(missing.runs.is_empty() && missing.reminders.is_empty());
}

async fn puts_replace_by_key_and_deletes_remove<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let mut moved = run("r-late", None, week(), 22);
    moved.status = RunStatus::Confirmed;
    let mut sent = reminder("m-3", "r-late", "day_of", 8);
    sent.sent_at = Some(at(8, 1));
    sent.message_id = Some("4242".into());
    commit(
        store,
        vec![
            Change::DeleteReminder("m-1".into()),
            Change::DeleteRsvp {
                run_id: "r-early".into(),
                user_id: "2".into(),
            },
            Change::DeleteFixedRun("f-b".into()),
            Change::DeleteReminder("never-existed".into()),
            Change::PutRun(moved.clone()),
            Change::PutReminder(sent.clone()),
            Change::PutRsvp(rsvp("r-early", "1", RsvpState::Maybe)),
        ],
    )
    .await
    .expect("replacing commit succeeds");
    let all = load(store, Scope::All).await;
    assert_eq!(ids(&all.fixed_runs, |row| &row.id), ["f-a"]);
    assert_eq!(all.runs.iter().find(|row| row.id == "r-late"), Some(&moved));
    assert_eq!(ids(&all.reminders, |row| &row.id), ["m-2", "m-3", "m-4"]);
    assert_eq!(
        all.reminders.iter().find(|row| row.id == "m-3"),
        Some(&sent)
    );
    let early: Vec<(&str, RsvpState)> = all
        .rsvps
        .iter()
        .filter(|row| row.run_id == "r-early")
        .map(|row| (row.user_id.as_str(), row.state))
        .collect();
    assert_eq!(early, [("1", RsvpState::Maybe)]);
}

async fn duplicate_reminder_kind_is_rejected_atomically<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let before = load(store, Scope::All).await;
    let result = commit(
        store,
        vec![
            Change::PutFixedRun(fixed("f-c", Weekday::Fri, 7)),
            Change::PutReminder(reminder("m-dup", "r-early", "day_of", 7)),
        ],
    )
    .await;
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "duplicate (run_id, kind): {result:?}"
    );
    assert_eq!(load(store, Scope::All).await, before, "nothing was written");
}

async fn duplicate_weekly_week_is_rejected_atomically<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let before = load(store, Scope::All).await;
    let result = commit(
        store,
        vec![
            Change::PutRsvp(rsvp("r-late", "1", RsvpState::Yes)),
            Change::PutRun(run("r-dup", Some("f-a"), week(), 14)),
        ],
    )
    .await;
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "duplicate (fixed_run_id, week_start): {result:?}"
    );
    assert_eq!(load(store, Scope::All).await, before, "nothing was written");
    commit(store, vec![Change::PutRun(run("r-free", None, week(), 14))])
        .await
        .expect("standalone runs never conflict");
}

async fn rebuilt_kind_in_one_commit_is_accepted<S: ScheduleStore>(store: &S) {
    seed(store).await;
    commit(
        store,
        vec![
            Change::DeleteReminder("m-2".into()),
            Change::PutReminder(reminder("m-new", "r-early", "day_of", 7)),
        ],
    )
    .await
    .expect("delete-then-put of one kind commits");
    let early = load(store, Scope::Run("r-early".into())).await;
    assert_eq!(ids(&early.reminders, |row| &row.id), ["m-new", "m-1"]);
}

async fn cross_week_swap_commits<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let next_week = week() + chrono::TimeDelta::days(7);
    let early = run("r-early", Some("f-a"), next_week, 10);
    let next = run("r-next", Some("f-a"), week(), 12);
    commit(
        store,
        vec![Change::PutRun(early.clone()), Change::PutRun(next.clone())],
    )
    .await
    .expect("cross_week_swap_commits: one weekly's runs swap boss weeks in one commit");
    let all = load(store, Scope::All).await;
    assert_eq!(
        all.runs.iter().find(|row| row.id == "r-early"),
        Some(&early)
    );
    assert_eq!(all.runs.iter().find(|row| row.id == "r-next"), Some(&next));
    let this = load(store, Scope::Weeks(vec![week()])).await;
    assert_eq!(ids(&this.runs, |row| &row.id), ["r-next", "r-late"]);
    assert_eq!(
        ids(&this.reminders, |row| &row.id),
        ["m-3", "m-4"],
        "reminders follow their run across weeks"
    );
}

async fn stale_revision_commit_conflicts<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let stale = load(store, Scope::Run("r-late".into())).await;
    let current = stale.revision;
    assert_eq!(
        load(store, Scope::All).await.revision,
        current,
        "every scope reads the one store revision"
    );
    store
        .commit(
            current,
            ChangeSet {
                changes: vec![Change::PutRsvp(rsvp("r-late", "1", RsvpState::Yes))],
            },
            meta(),
        )
        .await
        .expect("a commit at the current revision succeeds");
    let after = load(store, Scope::All).await;
    assert_eq!(
        after.revision,
        current + 1,
        "a commit advances the revision by one"
    );
    for changes in [
        vec![Change::PutRsvp(rsvp("r-late", "2", RsvpState::No))],
        Vec::new(),
    ] {
        let result = store.commit(current, ChangeSet { changes }, meta()).await;
        assert_eq!(
            result,
            Err(StoreError::Conflict {
                expected: current,
                found: current + 1,
            }),
            "stale_revision_commit_conflicts"
        );
    }
    assert_eq!(load(store, Scope::All).await, after, "nothing was written");
}

async fn rows_without_a_run_are_rejected_atomically<S: ScheduleStore>(store: &S) {
    seed(store).await;
    let before = load(store, Scope::All).await;
    for orphan in [
        Change::PutReminder(reminder("m-orphan", "absent", "day_of", 7)),
        Change::PutRsvp(rsvp("absent", "1", RsvpState::Yes)),
    ] {
        let result = commit(
            store,
            vec![Change::PutFixedRun(fixed("f-c", Weekday::Fri, 7)), orphan],
        )
        .await;
        assert!(
            matches!(result, Err(StoreError::Constraint(_))),
            "a row needs its run: {result:?}"
        );
        assert_eq!(load(store, Scope::All).await, before, "nothing was written");
    }
}

async fn instants_keep_microsecond_precision<S: ScheduleStore>(store: &S) {
    let ns = chrono::TimeDelta::nanoseconds;
    let mut row = run("r-fine", None, week() + ns(1_500), 10);
    row.datetime += ns(999);
    let mut ping = reminder("m-fine", "r-fine", "day_of", 9);
    ping.fire_at += ns(1);
    ping.sent_at = Some(at(9, 1) + ns(2_000_001));
    let mut answer = rsvp("r-fine", "1", RsvpState::Yes);
    answer.at += ns(1_234_567);
    commit(
        store,
        vec![
            Change::PutRun(row.clone()),
            Change::PutReminder(ping.clone()),
            Change::PutRsvp(answer.clone()),
        ],
    )
    .await
    .expect("sub-microsecond instants commit");
    row.week_start = week() + ns(1_000);
    row.datetime = at(10, 30);
    ping.fire_at = at(9, 0);
    ping.sent_at = Some(at(9, 1) + ns(2_000_000));
    answer.at = at(1, 0) + ns(1_234_000);
    let found = load(store, Scope::Weeks(vec![week() + ns(1_700)])).await;
    assert_eq!(found.runs, [row], "instants are stored to the microsecond");
    assert_eq!(found.reminders, [ping]);
    assert_eq!(found.rsvps, [answer]);

    let before = load(store, Scope::All).await;
    let mut timing = fixed("f-fine", Weekday::Mon, 21);
    timing.time += ns(500_000_000);
    let result = commit(store, vec![Change::PutFixedRun(timing)]).await;
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "a weekly time keeps whole seconds: {result:?}"
    );
    assert_eq!(load(store, Scope::All).await, before, "nothing was written");
}
