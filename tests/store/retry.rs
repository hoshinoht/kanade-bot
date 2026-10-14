//! The scheduler re-plans when another commit lands between load and commit.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use kanade::domain::ids::IdGenerator;
use kanade::domain::schedule::{Change, ChangeSet, NewFixedRun, ScheduleSnapshot};
use kanade::domain::scheduler::{
    COMMIT_ATTEMPTS, Clock, ScheduleStore, SchedulerError, SchedulerService, Scope, StoreError,
};
use kanade::infrastructure::store::SqliteStore;

use crate::support::{TempDir, at, fixed};

/// Lands a competing commit just before each of the next `remaining` commits.
struct Interfering {
    inner: SqliteStore,
    remaining: AtomicUsize,
    competitors: AtomicUsize,
}

impl ScheduleStore for Interfering {
    async fn recorded_request(
        &self,
        actor: &kanade::domain::history::Actor,
        request_id: &str,
    ) -> Result<Option<kanade::domain::scheduler::RecordedRequest>, StoreError> {
        self.inner.recorded_request(actor, request_id).await
    }

    async fn load(&self, scope: &Scope) -> Result<ScheduleSnapshot, StoreError> {
        self.inner.load(scope).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: kanade::domain::history::ChangeMeta,
    ) -> Result<Option<kanade::domain::scheduler::Committed>, StoreError> {
        if self
            .remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            let n = self.competitors.fetch_add(1, Ordering::SeqCst);
            let revision = self.inner.load(&Scope::All).await?.revision;
            let competitor = fixed(&format!("competitor-{n}"));
            self.inner
                .commit(
                    revision,
                    ChangeSet {
                        changes: vec![Change::PutFixedRun(competitor)],
                    },
                    kanade::infrastructure::store::conformance::meta(),
                )
                .await?;
        }
        self.inner.commit(expected_revision, changes, meta).await
    }
}

#[derive(Default)]
struct Ids(usize);

impl IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{}", self.0)
    }
}

struct Fixed(DateTime<Utc>);

impl Clock for Fixed {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn new_fixed() -> NewFixedRun {
    NewFixedRun {
        owner_pinned: false,
        owner_id: "42".into(),
        channel_id: None,
        bosses: vec!["HFA".into()],
        weekday: Weekday::Wed,
        time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
        participants: vec!["1".into()],
        note: None,
    }
}

async fn service(dir: &TempDir, interfere: usize) -> SchedulerService<Interfering, Ids, Fixed> {
    let inner = SqliteStore::open(&dir.config("retry"))
        .await
        .expect("opens");
    let store = Interfering {
        inner,
        remaining: AtomicUsize::new(interfere),
        competitors: AtomicUsize::new(0),
    };
    SchedulerService::new(store, Ids::default(), Fixed(at(26, 12)))
}

fn fixed_ids(snapshot: &ScheduleSnapshot) -> Vec<&str> {
    let mut ids: Vec<&str> = snapshot
        .fixed_runs
        .iter()
        .map(|row| row.id.as_str())
        .collect();
    ids.sort_unstable();
    ids
}

#[tokio::test]
async fn conflict_is_replanned_on_a_fresh_snapshot() {
    let dir = TempDir::new();
    let mut service = service(&dir, COMMIT_ATTEMPTS - 1).await;
    let id = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(new_fixed())
        .await
        .expect("retried");
    assert_eq!(id, "id-3", "each attempt re-planned and drew a new id");
    let state = service.store().load(&Scope::All).await.expect("load");
    assert_eq!(fixed_ids(&state), ["competitor-0", "competitor-1", "id-3"]);
    assert_eq!(state.revision, 3);
}

#[tokio::test]
async fn conflict_after_every_attempt_is_returned() {
    let dir = TempDir::new();
    let mut service = service(&dir, COMMIT_ATTEMPTS).await;
    let result = service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(new_fixed())
        .await;
    assert_eq!(
        result,
        Err(SchedulerError::Store(StoreError::Conflict {
            expected: 2,
            found: 3,
        }))
    );
    let state = service.store().load(&Scope::All).await.expect("load");
    assert_eq!(
        fixed_ids(&state),
        ["competitor-0", "competitor-1", "competitor-2"],
        "no planned write landed"
    );
}
