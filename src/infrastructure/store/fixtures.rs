//! Fixtures the conformance suites share: deterministic ids and clocks, the
//! UTC boss-week policy, the usual origins and weekly timing, and the
//! constraint-refusal assertion.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};

use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::ids::IdGenerator;
use crate::domain::schedule::{
    NewFixedRun, ReminderPolicy, RunStatus, SchedulePolicy, StatusChange,
};
use crate::domain::scheduler::{Clock, StoreError};

/// `id-0001`, `id-0002`, … shared by every clone.
#[derive(Clone, Default)]
pub(crate) struct Ids(Arc<Mutex<u64>>);

impl IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        let mut next = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *next += 1;
        format!("id-{next:04}")
    }
}

/// A clock a check can move; clones share the reading.
#[derive(Clone)]
pub(crate) struct TestClock(Arc<Mutex<DateTime<Utc>>>);

impl TestClock {
    pub(crate) fn at(now: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    pub(crate) fn advance(&self, by: TimeDelta) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A clock stopped at one instant.
#[derive(Clone, Copy)]
pub(crate) struct Stopped(pub(crate) DateTime<Utc>);

impl Clock for Stopped {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// An instant in 2026.
pub(crate) fn utc2026(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

/// Pings at 09:00 and one hour before, boss weeks from Thursday 00:00 in `zone`.
pub(crate) fn policy_in(zone: chrono_tz::Tz) -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
            countdowns: vec![60],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

/// The admin `root` on the admin portal.
pub(crate) fn admin() -> Origin {
    Origin::new(Actor::admin("root"), Surface::AdminPortal)
}

/// Member `id` on the public portal.
pub(crate) fn member(id: &str) -> Origin {
    Origin::new(Actor::member(id), Surface::PublicPortal)
}

/// The delivery tick, which materialises weeks.
pub(crate) fn delivery() -> Origin {
    Origin::new(Actor::system("delivery"), Surface::DeliveryTick)
}

/// A 20:00 HFA timing owned by member 1 in channel 900, party 1 and 2.
pub(crate) fn timing(weekday: Weekday) -> NewFixedRun {
    NewFixedRun {
        owner_pinned: false,
        owner_id: "1".into(),
        channel_id: Some("900".into()),
        bosses: vec!["HFA".into()],
        weekday,
        time: NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
        participants: vec!["1".into(), "2".into()],
        note: None,
    }
}

/// A quiet portal status change.
pub(crate) fn status(status: RunStatus) -> StatusChange {
    StatusChange {
        status,
        announce: false,
        via_portal: true,
    }
}

/// The store refused the write as a constraint violation.
#[track_caller]
pub(crate) fn refused<T: std::fmt::Debug>(result: Result<T, StoreError>) {
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "expected a constraint refusal, got {result:?}"
    );
}

/// [`refused`], labelled with the check that expects it.
#[track_caller]
pub(crate) fn refused_for<T: std::fmt::Debug>(
    check: impl std::fmt::Display,
    result: Result<T, StoreError>,
) {
    assert!(
        matches!(result, Err(StoreError::Constraint(_))),
        "{check}: expected a constraint refusal, got {result:?}"
    );
}
