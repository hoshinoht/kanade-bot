use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use axum::{http::StatusCode, response::IntoResponse};
use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use http_body_util::BodyExt;
use serde_json::Value;

use crate::{
    domain::{
        history::{
            Actor, BlameTarget, ChangeFilter, ChangeHistory, ChangeMeta, ChangeQuery, ChangeRecord,
            Expect, Origin, Precondition, Surface, changed_fields,
        },
        ids::IdGenerator,
        members::Roster,
        notify::{NoticeOutbox, OutboxNotice},
        schedule::{
            ChangeSet, FixedEdit, FixedEditChoices, FixedEditRequest, FixedRun, NewFixedRun,
            ReminderPolicy, SchedulePolicy, ScheduleSnapshot,
        },
        scheduler::{
            Clock, Committed, RecordedRequest, ScheduleStore, SchedulerError, SchedulerService,
            Scope, StoreError,
        },
    },
    infrastructure::store::MemoryScheduleStore,
};

const REQUEST_ID: &str = "fixed-patch-no-op-race";
const FIXED_TIME: NaiveTime = NaiveTime::from_hms_opt(21, 0, 0).unwrap();
const REQUESTED_TIME: NaiveTime = NaiveTime::from_hms_opt(20, 0, 0).unwrap();
const REQUESTED_NOTE: &str = "requested note";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Effects {
    schedule: ScheduleSnapshot,
    history: Vec<ChangeRecord>,
    outbox: Vec<OutboxNotice>,
}

async fn effects(store: &MemoryScheduleStore) -> Effects {
    Effects {
        schedule: store.load(&Scope::All).await.expect("schedule snapshot"),
        history: store
            .list_changes(&ChangeQuery::new(ChangeFilter::All))
            .await
            .expect("history page")
            .records,
        outbox: store.outbox_notices().await.expect("outbox notices"),
    }
}

#[derive(Clone)]
struct FixedClock(DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

struct CountingIds(u64);

impl IdGenerator for CountingIds {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("00000000-0000-4000-8000-{:012x}", self.0)
    }
}

fn identity(fixed_id: &str, note: &str, version: u64, expect: &Expect) -> String {
    format!(
        "FixedPatchForm {{ fixed_id: {fixed_id:?}, time: {REQUESTED_TIME:?}, note: {note:?}, version: {version}, expect: {expect:?} }}"
    )
}

fn request(fixed_id: &str) -> FixedEditRequest {
    FixedEditRequest {
        fixed_id: fixed_id.to_owned(),
        edit: FixedEdit {
            note: Some(REQUESTED_NOTE.to_owned()),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::PerRun(Default::default()),
    }
}

fn competing_request(fixed_id: &str) -> FixedEditRequest {
    FixedEditRequest {
        fixed_id: fixed_id.to_owned(),
        edit: FixedEdit {
            time: Some(REQUESTED_TIME),
            note: Some(REQUESTED_NOTE.to_owned()),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::PerRun(Default::default()),
    }
}

fn origin() -> Origin {
    Origin::new(Actor::admin("proof-admin"), Surface::AdminPortal).with_request_id(REQUEST_ID)
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Mon,
        NaiveTime::MIN,
    )
}

struct PatchAttempt<'a> {
    origin: Origin,
    expect: Expect,
    patch: &'a FixedEditRequest,
    identity: &'a str,
    policy: &'a SchedulePolicy,
}

async fn apply_fixed_patch<S: ScheduleStore, I: IdGenerator, C: Clock>(
    store: S,
    ids: I,
    clock: C,
    attempt: PatchAttempt<'_>,
    directory: &Roster,
) -> Result<crate::domain::schedule::Outcome<FixedRun>, SchedulerError> {
    let mut service = SchedulerService::new(store, ids, clock);
    service
        .as_origin(attempt.origin)
        .expecting(attempt.expect)
        .apply_fixed_patch_edit(attempt.patch, attempt.identity, directory, attempt.policy)
        .await
}

struct CompetingWrite {
    origin: Origin,
    expect: Expect,
    patch: FixedEditRequest,
    identity: String,
    clock: FixedClock,
    policy: SchedulePolicy,
}

struct InterleavingStore {
    inner: Arc<MemoryScheduleStore>,
    competing: Mutex<Option<CompetingWrite>>,
    competing_baseline: Mutex<Option<Effects>>,
    request_reads: AtomicUsize,
    request_reads_at_injection: AtomicUsize,
    injections: AtomicUsize,
    empty_commits: AtomicUsize,
}

impl InterleavingStore {
    fn new(inner: Arc<MemoryScheduleStore>, competing: Option<CompetingWrite>) -> Self {
        Self {
            inner,
            competing: Mutex::new(competing),
            competing_baseline: Mutex::new(None),
            request_reads: AtomicUsize::new(0),
            request_reads_at_injection: AtomicUsize::new(0),
            injections: AtomicUsize::new(0),
            empty_commits: AtomicUsize::new(0),
        }
    }

    fn baseline(&self) -> Effects {
        self.competing_baseline
            .lock()
            .unwrap()
            .clone()
            .expect("competing write captured its baseline")
    }
}

impl ScheduleStore for InterleavingStore {
    async fn load(&self, scope: &Scope) -> Result<ScheduleSnapshot, StoreError> {
        let competing = self.competing.lock().unwrap().take();
        if let Some(competing) = competing {
            self.request_reads_at_injection
                .store(self.request_reads.load(Ordering::SeqCst), Ordering::SeqCst);
            let directory = Roster::new();
            let result = apply_fixed_patch(
                self.inner.as_ref(),
                CountingIds(100),
                competing.clock,
                PatchAttempt {
                    origin: competing.origin,
                    expect: competing.expect,
                    patch: &competing.patch,
                    identity: &competing.identity,
                    policy: &competing.policy,
                },
                &directory,
            )
            .await;
            assert!(
                result.is_ok(),
                "competing fixed PATCH should commit: {result:?}"
            );
            self.injections.fetch_add(1, Ordering::SeqCst);
            let baseline = effects(self.inner.as_ref()).await;
            *self.competing_baseline.lock().unwrap() = Some(baseline);
        }
        self.inner.load(scope).await
    }

    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> Result<Option<RecordedRequest>, StoreError> {
        self.request_reads.fetch_add(1, Ordering::SeqCst);
        self.inner.recorded_request(actor, request_id).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> Result<Option<Committed>, StoreError> {
        if changes.changes.is_empty() {
            self.empty_commits.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.commit(expected_revision, changes, meta).await
    }
}

struct Fixture {
    store: Arc<MemoryScheduleStore>,
    fixed_id: String,
    clock: FixedClock,
    policy: SchedulePolicy,
    expect: Expect,
    before: Effects,
}

async fn fixture() -> Fixture {
    let store = Arc::new(MemoryScheduleStore::new());
    let clock = FixedClock(Utc.with_ymd_and_hms(2026, 10, 6, 4, 0, 0).single().unwrap());
    let mut service = SchedulerService::new(Arc::clone(&store), CountingIds(0), clock.clone());
    let fixed_id = service
        .as_origin(Origin::new(Actor::admin("seed"), Surface::AdminPortal))
        .add_fixed_run(NewFixedRun {
            owner_id: "1001".into(),
            channel_id: Some("test-channel".into()),
            bosses: vec!["XKalos".into()],
            weekday: Weekday::Tue,
            time: FIXED_TIME,
            participants: vec!["1001".into()],
            note: Some("seed".into()),
        })
        .await
        .expect("seed weekly timing");
    let before = effects(store.as_ref()).await;
    let seen = store.history_head().await.expect("seed history head").seq;
    let expect = Expect::fields([Precondition::new(
        BlameTarget::FixedRun(fixed_id.clone()),
        "time",
        Some(seen),
    )]);
    Fixture {
        store,
        fixed_id,
        clock,
        policy: policy(),
        expect,
        before,
    }
}

fn competing_write(fixture: &Fixture, identity: String) -> CompetingWrite {
    CompetingWrite {
        origin: origin(),
        expect: fixture.expect.clone(),
        patch: competing_request(&fixture.fixed_id),
        identity,
        clock: fixture.clock.clone(),
        policy: fixture.policy.clone(),
    }
}

async fn assert_api_mismatch(error: SchedulerError) {
    let response = super::scheduler(error).into_response();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "idempotency_mismatch");
}

fn assert_one_competing_change(before: &Effects, after: &Effects) {
    assert_eq!(after.schedule.revision, before.schedule.revision + 1);
    assert_eq!(after.history.len(), before.history.len() + 1);
    assert_eq!(after.outbox.len(), before.outbox.len() + 1);
}

#[tokio::test]
async fn different_competing_identity_on_fixed_patch_no_op_returns_api_422_without_effects() {
    let fixture = fixture().await;
    let patch = request(&fixture.fixed_id);
    let main_identity = identity(&fixture.fixed_id, REQUESTED_NOTE, 7, &fixture.expect);
    let different_identity = identity(&fixture.fixed_id, REQUESTED_NOTE, 8, &fixture.expect);
    let store = Arc::new(InterleavingStore::new(
        Arc::clone(&fixture.store),
        Some(competing_write(&fixture, different_identity)),
    ));
    let result = apply_fixed_patch(
        Arc::clone(&store),
        CountingIds(200),
        fixture.clock.clone(),
        PatchAttempt {
            origin: origin(),
            expect: fixture.expect.clone(),
            patch: &patch,
            identity: &main_identity,
            policy: &fixture.policy,
        },
        &Roster::new(),
    )
    .await;

    let error = result.expect_err("a competing different request identity must be refused");
    assert!(matches!(error, SchedulerError::IdempotencyMismatch { .. }));
    assert_api_mismatch(error).await;

    assert_eq!(
        store.request_reads_at_injection.load(Ordering::SeqCst),
        2,
        "the competitor lands after both fixed-PATCH request checks"
    );
    assert_eq!(store.injections.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.empty_commits.load(Ordering::SeqCst),
        1,
        "the main operation reached the empty commit with a time expectation"
    );
    assert_eq!(fixture.expect.fields.len(), 1);
    assert_eq!(fixture.expect.fields[0].field, "time");
    assert_eq!(
        fixture.expect.fields[0].seen,
        Some(fixture.before.schedule.revision)
    );

    let baseline = store.baseline();
    assert_one_competing_change(&fixture.before, &baseline);
    let competing_record = baseline.history.last().expect("competing history record");
    assert_ne!(fixture.expect.fields[0].seen, Some(competing_record.seq));
    assert!(changed_fields(competing_record).contains(&(
        BlameTarget::FixedRun(fixture.fixed_id.clone()),
        "time".into()
    )));
    assert_eq!(
        baseline.schedule.fixed_runs[0].time, REQUESTED_TIME,
        "the competing writer moved time to the requested field value"
    );
    assert_eq!(
        baseline.schedule.fixed_runs[0].note.as_deref(),
        Some(REQUESTED_NOTE)
    );
    assert_eq!(effects(fixture.store.as_ref()).await, baseline);
}

#[tokio::test]
async fn exact_competing_identity_on_fixed_patch_no_op_replays_without_extra_effects() {
    let fixture = fixture().await;
    let patch = request(&fixture.fixed_id);
    let exact_identity = identity(&fixture.fixed_id, REQUESTED_NOTE, 7, &fixture.expect);
    let store = Arc::new(InterleavingStore::new(
        Arc::clone(&fixture.store),
        Some(competing_write(&fixture, exact_identity.clone())),
    ));
    let result = apply_fixed_patch(
        Arc::clone(&store),
        CountingIds(200),
        fixture.clock.clone(),
        PatchAttempt {
            origin: origin(),
            expect: fixture.expect.clone(),
            patch: &patch,
            identity: &exact_identity,
            policy: &fixture.policy,
        },
        &Roster::new(),
    )
    .await
    .expect("an exact competing identity is a replay");

    assert_eq!(result.value.note.as_deref(), Some(REQUESTED_NOTE));
    assert_eq!(store.request_reads_at_injection.load(Ordering::SeqCst), 2);
    assert_eq!(store.injections.load(Ordering::SeqCst), 1);
    assert_eq!(store.empty_commits.load(Ordering::SeqCst), 1);
    let baseline = store.baseline();
    assert_one_competing_change(&fixture.before, &baseline);
    assert_eq!(effects(fixture.store.as_ref()).await, baseline);
}

#[tokio::test]
async fn fresh_fixed_patch_commits_once() {
    let fixture = fixture().await;
    let patch = request(&fixture.fixed_id);
    let fresh_identity = identity(&fixture.fixed_id, REQUESTED_NOTE, 7, &fixture.expect);
    let store = Arc::new(InterleavingStore::new(Arc::clone(&fixture.store), None));
    let result = apply_fixed_patch(
        Arc::clone(&store),
        CountingIds(100),
        fixture.clock.clone(),
        PatchAttempt {
            origin: origin(),
            expect: fixture.expect.clone(),
            patch: &patch,
            identity: &fresh_identity,
            policy: &fixture.policy,
        },
        &Roster::new(),
    )
    .await
    .expect("a fresh fixed PATCH commits");

    assert_eq!(result.value.note.as_deref(), Some(REQUESTED_NOTE));
    assert_eq!(store.injections.load(Ordering::SeqCst), 0);
    assert_eq!(store.empty_commits.load(Ordering::SeqCst), 0);
    let after = effects(fixture.store.as_ref()).await;
    assert_one_competing_change(&fixture.before, &after);
    assert_eq!(
        after.schedule.fixed_runs[0].note.as_deref(),
        Some(REQUESTED_NOTE)
    );
}
