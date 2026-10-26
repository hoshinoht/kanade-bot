//! Support shared by several test targets: pinned clock/id seams, a
//! scheduler over a temp SQLite store, JSON field readers and structural
//! schedule checks. Each target uses a different subset.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, NaiveTime, Utc};
use chrono_tz::Tz;
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::{
    ids::IdGenerator,
    schedule::ScheduleSnapshot,
    scheduler::{Clock, ScheduleStore, SchedulerService, Scope},
    time::{IsoDateTime, to_iso},
    weeks,
};
use serde_json::Value;

pub type Service<S = SqliteStore> = SchedulerService<S, SeqIds, TestClock>;

/// A fixed UUID sequence; running out fails the test.
/// Clones share one cursor so a test can draw ids the service did not.
#[derive(Clone)]
pub struct SeqIds {
    ids: Arc<Vec<String>>,
    next: Arc<Mutex<usize>>,
}

impl SeqIds {
    pub fn new(ids: &Value) -> Self {
        Self {
            ids: Arc::new(strings(ids)),
            next: Arc::new(Mutex::new(0)),
        }
    }
}

impl IdGenerator for SeqIds {
    fn new_id(&mut self) -> String {
        let mut next = self.next.lock().unwrap();
        let id = self
            .ids
            .get(*next)
            .unwrap_or_else(|| panic!("uuid_sequence exhausted after {} ids", *next))
            .clone();
        *next += 1;
        id
    }
}

/// One aware clock shared by the service and the test (`set`).
#[derive(Clone)]
pub struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    pub fn new(at: DateTime<FixedOffset>) -> Self {
        Self(Arc::new(Mutex::new(at.with_timezone(&Utc))))
    }

    pub fn set(&self, at: DateTime<FixedOffset>) {
        *self.0.lock().unwrap() = at.with_timezone(&Utc);
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

pub async fn service(input: &Value) -> (Service, TestClock) {
    let (service, clock, _) = service_with_ids(input).await;
    (service, clock)
}

/// A scheduler at `input.clock` drawing ids from `input.uuid_sequence`.
pub async fn service_with_ids(input: &Value) -> (Service, TestClock, SeqIds) {
    service_over(SqliteStore::open_temp().await, input)
}

/// [`service_with_ids`] over `store`.
pub fn service_over<S: ScheduleStore>(store: S, input: &Value) -> (Service<S>, TestClock, SeqIds) {
    let clock = TestClock::new(instant(&input["clock"]));
    let ids = SeqIds::new(&input["uuid_sequence"]);
    let service = SchedulerService::new(store, ids.clone(), clock.clone());
    (service, clock, ids)
}

pub fn text(value: &Value) -> &str {
    value
        .as_str()
        .unwrap_or_else(|| panic!("{value} must be a string"))
}

pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("{value} must be an array"))
        .iter()
        .map(|item| text(item).to_owned())
        .collect()
}

pub fn instant(value: &Value) -> DateTime<FixedOffset> {
    match IsoDateTime::parse(text(value)).expect("aware datetime") {
        IsoDateTime::Aware(at) => at,
        IsoDateTime::Naive(_) => panic!("{value} must be aware"),
    }
}

pub fn utc(value: &Value) -> DateTime<Utc> {
    instant(value).with_timezone(&Utc)
}

pub fn zone(input: &Value) -> Tz {
    text(&input["timezone"]).parse().expect("IANA timezone")
}

/// `HH:MM` or `HH:MM:SS`.
pub fn clock_time(value: &Value) -> NaiveTime {
    let parts: Vec<u32> = text(value)
        .split(':')
        .map(|part| part.parse().expect("clock digits"))
        .collect();
    match parts[..] {
        [hour, minute] => NaiveTime::from_hms_opt(hour, minute, 0),
        [hour, minute, second] => NaiveTime::from_hms_opt(hour, minute, second),
        _ => None,
    }
    .unwrap_or_else(|| panic!("{value} must be a clock time"))
}

pub fn weekday(value: &Value) -> chrono::Weekday {
    weeks::weekday_from_index(value.as_i64().expect("weekday index")).expect("weekday 0..=6")
}

pub fn countdowns(value: &Value) -> Vec<u32> {
    value
        .as_array()
        .expect("countdowns array")
        .iter()
        .map(|m| u32::try_from(m.as_u64().expect("minutes")).expect("u32 minutes"))
        .collect()
}

pub fn iso(at: DateTime<Utc>) -> String {
    to_iso(&at).expect("in range")
}

pub async fn snapshot<S: ScheduleStore>(service: &Service<S>) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.expect("load")
}

/// Structural checks that must hold after any mutation: reminder kinds unique
/// per run, no pending pings on done/cancelled runs, and no already-due ping
/// on the `moved` runs (`None` = every run; older runs may otherwise carry a
/// legitimate backlog from ticks that never ran).
pub fn assert_sound(state: &ScheduleSnapshot, now: DateTime<Utc>, moved: Option<&[&String]>) {
    let mut kinds: Vec<(&str, &str)> = state
        .reminders
        .iter()
        .map(|row| (row.run_id.as_str(), row.kind.as_str()))
        .collect();
    let total = kinds.len();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds.len(), total, "duplicate (run, kind) reminders");
    let due: Vec<&str> = state
        .reminders
        .iter()
        .filter(|row| moved.is_none_or(|runs| runs.contains(&&row.run_id)))
        .filter(|row| row.sent_at.is_none() && row.fire_at <= now)
        .map(|row| row.id.as_str())
        .collect();
    assert!(due.is_empty(), "stale due reminders {due:?}");
    for run in state.runs.iter().filter(|run| run.status.is_terminal()) {
        assert!(
            !state
                .reminders
                .iter()
                .any(|row| row.run_id == run.id && row.sent_at.is_none()),
            "terminal run {} keeps pending pings",
            run.id
        );
    }
}
