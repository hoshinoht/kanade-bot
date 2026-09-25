use std::fmt;

use chrono::{DateTime, Datelike, FixedOffset, NaiveTime, TimeDelta, Weekday};
use chrono_tz::Tz;

use crate::domain::pytext::{repr, strip};
use crate::domain::time::{DateOutOfRange, ZonedDateTime};
use crate::domain::weeks;

/// What `/rescan window:` offers, longest first.
pub const WINDOWS: [&str; 4] = ["week", "2weeks", "48h", "24h"];

pub const DEFAULT_WINDOW: &str = "week";

/// The furthest back the bot may look on its own initiative.
pub const AUTOMATED_WINDOW: &str = "48h";

/// Fixed-length windows; `week`/`2weeks` are boss weeks, not 168/336 hours.
const HOUR_WINDOWS: [(&str, i64); 2] = [("24h", 24), ("48h", 48)];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowError {
    /// Python `ValueError`, keeping the window as given.
    Unknown { window: String },
    /// Python `OverflowError`.
    OutOfRange,
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { window } => write!(
                f,
                "unknown window {}; expected one of {}",
                repr(window),
                WINDOWS.join(", ")
            ),
            Self::OutOfRange => DateOutOfRange.fmt(f),
        }
    }
}

impl std::error::Error for WindowError {}

impl From<DateOutOfRange> for WindowError {
    fn from(_: DateOutOfRange) -> Self {
        Self::OutOfRange
    }
}

fn key(window: &str) -> String {
    let window = if window.is_empty() {
        DEFAULT_WINDOW
    } else {
        window
    };
    strip(window).to_lowercase()
}

fn unknown(window: &str) -> WindowError {
    WindowError::Unknown {
        window: window.to_owned(),
    }
}

/// The window that will actually be read: automated rescans are capped at
/// [`AUTOMATED_WINDOW`] here so no caller can widen one by accident.
///
/// # Errors
/// [`WindowError::Unknown`] for anything not in [`WINDOWS`].
pub fn clamp_window(window: &str, automated: bool) -> Result<&'static str, WindowError> {
    let key = key(window);
    let known = WINDOWS
        .into_iter()
        .find(|name| *name == key)
        .ok_or_else(|| unknown(window))?;
    if !automated || HOUR_WINDOWS.iter().any(|(name, _)| *name == known) {
        Ok(known)
    } else {
        Ok(AUTOMATED_WINDOW)
    }
}

/// The instant a rescan window starts at: `week` is the current boss week's
/// reset, `2weeks` the one before; hour windows keep `now`'s offset.
///
/// # Errors
/// [`WindowError::Unknown`] for an unknown window (after the week start is
/// computed, as v4 does), [`WindowError::OutOfRange`] outside years 1..=9999.
pub fn window_since(
    window: &str,
    zone: Tz,
    reset_weekday: Weekday,
    reset_time: NaiveTime,
    now: &DateTime<FixedOffset>,
) -> Result<DateTime<FixedOffset>, WindowError> {
    let key = key(window);
    if let Some(&(_, hours)) = HOUR_WINDOWS.iter().find(|(name, _)| *name == key) {
        return now
            .checked_sub_signed(TimeDelta::hours(hours))
            .filter(|since| (1..=9999).contains(&since.year()))
            .ok_or(WindowError::OutOfRange);
    }
    let this_week = weeks::week_start(now, zone, reset_weekday, reset_time)?;
    match key.as_str() {
        "week" => Ok(this_week.to_fixed()),
        "2weeks" => {
            Ok(previous_week_start(&this_week, zone, reset_weekday, reset_time)?.to_fixed())
        }
        _ => Err(unknown(window)),
    }
}

/// The reset before `this_week`: one second earlier on its wall clock, then
/// back to that week's start.
///
/// # Errors
/// [`DateOutOfRange`] outside years 1..=9999.
pub fn previous_week_start(
    this_week: &ZonedDateTime,
    zone: Tz,
    reset_weekday: Weekday,
    reset_time: NaiveTime,
) -> Result<ZonedDateTime, DateOutOfRange> {
    let wall = this_week
        .wall()
        .checked_sub_signed(TimeDelta::seconds(1))
        .ok_or(DateOutOfRange)?;
    let before = ZonedDateTime::new(wall, this_week.zone())?;
    weeks::week_start(&before, zone, reset_weekday, reset_time)
}

/// Look back one more boss week only from a manual `week` rescan that found
/// no scheduling chat at all.
pub fn should_widen(window: &str, gated_count: usize, automated: bool) -> bool {
    !automated && window == "week" && gated_count == 0
}
