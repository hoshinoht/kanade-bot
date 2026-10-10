//! Guild scheduling settings and the week-start conversions planners share.

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;

use super::reminders::ReminderPolicy;
use crate::domain::attendance::AttendancePolicy;
use crate::domain::time::{AwareDateTime, DateOutOfRange, ZonedDateTime};
use crate::domain::weeks;

/// Reminder settings, the boss-week reset and the attendance rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchedulePolicy {
    pub reminders: ReminderPolicy,
    pub reset_weekday: Weekday,
    pub reset_time: NaiveTime,
    /// v4-compatible unless opted in ([`Self::with_attendance`]).
    pub attendance: AttendancePolicy,
}

impl SchedulePolicy {
    /// A policy with v4 attendance ([`AttendancePolicy::V4_COMPAT`]).
    pub fn new(reminders: ReminderPolicy, reset_weekday: Weekday, reset_time: NaiveTime) -> Self {
        Self {
            reminders,
            reset_weekday,
            reset_time,
            attendance: AttendancePolicy::V4_COMPAT,
        }
    }

    #[must_use]
    pub fn with_attendance(mut self, attendance: AttendancePolicy) -> Self {
        self.attendance = attendance;
        self
    }

    pub fn zone(&self) -> Tz {
        self.reminders.zone
    }

    /// The boss week containing `at`, keeping a reset wall clock that falls in
    /// a DST gap (as v4 does).
    ///
    /// # Errors
    /// [`DateOutOfRange`].
    pub fn week_of(&self, at: &impl AwareDateTime) -> Result<ZonedDateTime, DateOutOfRange> {
        weeks::week_start(at, self.zone(), self.reset_weekday, self.reset_time)
    }

    /// The current and next two boss weeks, the ones materialisation keeps.
    ///
    /// # Errors
    /// [`DateOutOfRange`].
    pub fn materialised_weeks(
        &self,
        now: DateTime<Utc>,
    ) -> Result<[ZonedDateTime; 3], DateOutOfRange> {
        weeks::materialised_week_starts(self.zone(), self.reset_weekday, self.reset_time, &now)
    }
}

/// The stored UTC instant of an aware week start (v4 `to_iso`). A zoned gap
/// wall clock resolves with its pre-transition offset.
///
/// # Errors
/// [`DateOutOfRange`].
pub fn utc_instant(at: &impl AwareDateTime) -> Result<DateTime<Utc>, DateOutOfRange> {
    Ok(at
        .astimezone(chrono_tz::UTC)?
        .to_fixed()
        .with_timezone(&Utc))
}
