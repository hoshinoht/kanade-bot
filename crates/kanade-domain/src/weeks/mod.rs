//! Boss-week and calendar-week arithmetic plus fixed-slot placement.
//!
//! A boss week starts at the configured reset (weekday and wall-clock time in the
//! guild zone) and ends at the same wall clock seven days later. A calendar week
//! runs Monday to Monday midnight. Keep the two distinct: date language reads
//! calendar weeks while storage is bucketed by boss week.

mod boundaries;
mod parse;

pub use boundaries::{
    calendar_week_end, calendar_week_start, materialised_week_starts, slot_in_week, week_end,
    week_start,
};
pub use parse::{WeekParseError, parse_hhmm, parse_weekday, weekday_from_index};
