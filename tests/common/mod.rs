//! Scheduler-vector support shared by the `scheduler` and `notify` test
//! targets: vector loading, pinned clock/id seams and v4-shaped snapshots.
//! Each target uses a different subset.
#![allow(dead_code)]

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use chrono::{DateTime, FixedOffset, NaiveTime, Timelike, Utc};
use chrono_tz::Tz;
use kanade::domain::{
    ids::IdGenerator,
    schedule::{Reminder, ScheduleSnapshot},
    scheduler::{Clock, ScheduleStore, SchedulerService, Scope},
    time::{IsoDateTime, to_iso},
    weeks,
};
use kanade::infrastructure::store::MemoryScheduleStore;
use serde_json::{Value, json};

pub type Service = SchedulerService<MemoryScheduleStore, SeqIds, TestClock>;

pub fn load(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/v5/vectors/scheduler")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path:?}: {error}"))
}

/// The case's UUID sequence; running out fails the contract, as in the producer.
/// Clones share one cursor so the replayer can draw ids v4 spent elsewhere.
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

/// One aware clock shared by the service and the replayer (`set_clock`).
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

pub fn service(input: &Value) -> (Service, TestClock) {
    let (service, clock, _) = service_with_ids(input);
    (service, clock)
}

pub fn service_with_ids(input: &Value) -> (Service, TestClock, SeqIds) {
    let clock = TestClock::new(instant(&input["clock"]));
    let ids = SeqIds::new(&input["uuid_sequence"]);
    let service = SchedulerService::new(MemoryScheduleStore::new(), ids.clone(), clock.clone());
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
    match IsoDateTime::parse(text(value)).expect("vector datetime") {
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

pub fn opt_iso(at: Option<DateTime<Utc>>) -> Value {
    at.map_or(Value::Null, |at| json!(iso(at)))
}

pub fn hhmm(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

pub fn reminder_json(row: &Reminder, message_id: bool) -> Value {
    let mut value = json!({
        "id": row.id,
        "run_id": row.run_id,
        "kind": row.kind,
        "fire_at": iso(row.fire_at),
        "sent_at": opt_iso(row.sent_at),
    });
    if message_id {
        value["message_id"] = json!(row.message_id);
    }
    value
}

pub async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.expect("load")
}

/// The v4 replayer's `final_state`: reminders grouped by run order, then `(fire_at, kind)`.
pub fn final_state(state: &ScheduleSnapshot, message_id: bool) -> Value {
    let mut by_run: BTreeMap<&str, Vec<&Reminder>> = BTreeMap::new();
    for row in &state.reminders {
        by_run.entry(&row.run_id).or_default().push(row);
    }
    let reminders: Vec<Value> = state
        .runs
        .iter()
        .flat_map(|run| {
            let mut rows = by_run.remove(run.id.as_str()).unwrap_or_default();
            rows.sort_by(|a, b| (a.fire_at, &a.kind).cmp(&(b.fire_at, &b.kind)));
            rows.into_iter().map(|row| reminder_json(row, message_id))
        })
        .collect();
    assert!(by_run.is_empty(), "reminders without a run: {by_run:?}");
    json!({
        "fixed_runs": state.fixed_runs.iter().map(|row| json!({
            "id": row.id,
            "owner_id": row.owner_id,
            "channel_id": row.channel_id,
            "bosses": row.bosses,
            "weekday": row.weekday.num_days_from_monday(),
            "time": hhmm(row.time),
            "participants": row.participants,
            "note": row.note,
        })).collect::<Vec<_>>(),
        "runs": state.runs.iter().map(|row| json!({
            "id": row.id,
            "fixed_run_id": row.fixed_run_id,
            "channel_id": row.channel_id,
            "bosses": row.bosses,
            "participants": row.participants,
            "status": row.status.as_str(),
            "source": row.source.as_str(),
            "week_start": iso(row.week_start),
            "datetime": iso(row.datetime),
        })).collect::<Vec<_>>(),
        "reminders": reminders,
        "rsvps": state.rsvps.iter().map(|row| json!({
            "run_id": row.run_id,
            "user_id": row.user_id,
            "state": row.state.as_str(),
            "source": row.source.as_str(),
            "at": iso(row.at),
        })).collect::<Vec<_>>(),
        "side_effects": [],
    })
}

/// Register `"{fixed_key}@{week_text}"` for every timing materialised that week.
pub async fn register_materialised(
    service: &Service,
    fixed_refs: &BTreeMap<String, String>,
    week: DateTime<Utc>,
    week_text: &str,
    run_refs: &mut BTreeMap<String, String>,
) {
    let state = snapshot(service).await;
    for (key, fixed_id) in fixed_refs {
        if let Some(run) = state
            .runs
            .iter()
            .find(|run| run.fixed_run_id.as_ref() == Some(fixed_id) && run.week_start == week)
        {
            run_refs.insert(format!("{key}@{week_text}"), run.id.clone());
        }
    }
}

pub fn run_ref<'a>(refs: &'a BTreeMap<String, String>, key: &Value) -> &'a str {
    refs.get(text(key))
        .unwrap_or_else(|| panic!("no run produced for key {key}"))
}

/// A step's result: its value, or the declared `ValueError` with v4's message.
pub fn step_result<E: std::fmt::Display>(step: &Value, result: Result<Value, E>) -> Value {
    match (result, step.get("error_type")) {
        (Ok(value), _) => json!({ "value": value }),
        (Err(error), Some(kind)) => {
            json!({ "error": { "type": kind, "message": error.to_string() } })
        }
        (Err(error), None) => panic!("step {step} failed unexpectedly: {error}"),
    }
}

/// Compare one replayed case to its expectation, listing every difference.
pub fn assert_case(case_id: &str, actual: &Value, expected: &Value) {
    let mut failures = Vec::new();
    let steps = actual["steps"].as_array().expect("steps");
    let wanted = expected["steps"].as_array().expect("expected steps");
    assert_eq!(steps.len(), wanted.len(), "{case_id}: step count");
    for (index, (got, want)) in steps.iter().zip(wanted).enumerate() {
        if got != want {
            failures.push(format!("step {index}: expected {want}\n  got {got}"));
        }
    }
    for table in ["fixed_runs", "runs", "reminders", "rsvps", "side_effects"] {
        let (got, want) = (
            &actual["final_state"][table],
            &expected["final_state"][table],
        );
        if got != want {
            failures.push(format!("final {table}: expected {want}\n  got {got}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{case_id} mismatches:\n{}",
        failures.join("\n")
    );
}

/// A frozen v4 value the user deliberately changed for v5. Each entry must
/// match the vector's v4 value before it is replaced, and every entry of a
/// named list must be used (callers sum [`apply_deviations`] counts).
pub struct Deviation {
    pub case_id: &'static str,
    pub pointer: String,
    pub v4: Value,
    pub v5: Value,
}

/// The expectation after the named deviations; returns how many applied.
pub fn apply_deviations(
    case_id: &str,
    expected: &Value,
    deviations: &[Deviation],
) -> (Value, usize) {
    let mut expected = expected.clone();
    let mut applied = 0;
    for deviation in deviations.iter().filter(|d| d.case_id == case_id) {
        let slot = expected
            .pointer_mut(&deviation.pointer)
            .unwrap_or_else(|| panic!("{case_id}: no {}", deviation.pointer));
        assert_eq!(
            *slot, deviation.v4,
            "{case_id}{}: frozen v4 value changed",
            deviation.pointer
        );
        *slot = deviation.v5.clone();
        applied += 1;
    }
    (expected, applied)
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
