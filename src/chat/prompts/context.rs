//! Per-turn trusted context lines, rendered exactly as v4 does.

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, TimeZone, Timelike};
use chrono_tz::Tz;

pub const FOCUS_PREFIX: &str = "The last card posted in this channel: ";
pub const FOCUS_SUFFIX: &str = ". If somebody says \"it\" or \"that run\" with nothing else to point at, that is what they mean. It is still only a proposal until somebody reacts ✅ on it.";

pub(super) const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
pub(super) const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `%A %d %B`
fn day(date: NaiveDate) -> String {
    format!(
        "{} {:02} {}",
        WEEKDAYS[date.weekday().num_days_from_monday() as usize],
        date.day(),
        MONTHS[date.month0() as usize]
    )
}

/// `%A %d %B %H:%M`
fn day_time(at: NaiveDateTime) -> String {
    format!("{} {:02}:{:02}", day(at.date()), at.hour(), at.minute())
}

/// Local time plus calendar and boss-week context. `week_start` is the current
/// boss-week reset; its end is seven wall-clock days later, as in v4.
pub fn clock_header<A: TimeZone, B: TimeZone>(
    now: &DateTime<A>,
    zone: Tz,
    week_start: &DateTime<B>,
) -> String {
    let local = now.with_timezone(&zone).naive_local();
    let week = week_start.with_timezone(&zone).naive_local();
    let today = local.date();
    let calendar_start = today - TimeDelta::days(i64::from(today.weekday().num_days_from_monday()));
    let calendar_end = calendar_start + TimeDelta::days(6);
    let reset_end = week + TimeDelta::days(7);
    format!(
        "Right now it is {} {}, {:02}:{:02} ({}). The calendar week is {} to {}. \
         The current boss week runs from {} to {}. Unqualified 'this week' and \
         'next week' mean calendar weeks.",
        day(today),
        today.year(),
        local.hour(),
        local.minute(),
        zone.name(),
        day(calendar_start),
        day(calendar_end),
        day_time(week),
        day_time(reset_end),
    )
}

/// Runtime-model fact line; an unset model is named as such, never invented.
pub fn runtime_line(model: &str) -> String {
    let named = crate::chat::persona::py_strip(model);
    let model = if named.is_empty() {
        "an unnamed model".to_owned()
    } else {
        format!("the `{named}` model")
    };
    format!(
        "You are a Discord bot for this guild's boss schedule. You run on {model}, served \
         through Kanata. If somebody asks what you run on or what model you are, that is \
         the fact: say it in your own voice. Never invent a model name, a version or a \
         training story, and never claim to be anything other than a bot."
    )
}

/// Channel-scoped current-card context, or `""` when there is no card.
pub fn focus_line(card: &str) -> String {
    let text = crate::chat::persona::py_strip(card);
    if text.is_empty() {
        return String::new();
    }
    format!("{FOCUS_PREFIX}{text}{FOCUS_SUFFIX}")
}
