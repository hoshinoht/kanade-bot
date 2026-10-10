//! v4 `bot.agent.formatting` texts the commands reply with, byte for byte.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;

use crate::domain::catalog::BossTable;
use crate::domain::ids::short_id;
use crate::domain::schedule::{FixedRun, RsvpState, Run, RunStatus};

pub const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn mention(user_id: &str) -> String {
    format!("<@{user_id}>")
}

/// v4 `format_bosses`: stored tokens, not labels.
pub fn format_bosses(bosses: &[String]) -> String {
    if bosses.is_empty() {
        "(no bosses)".to_owned()
    } else {
        bosses.join(" + ")
    }
}

/// v4 `format_participants` without an audience: everyone as a mention.
pub fn format_participants(user_ids: &[String]) -> String {
    if user_ids.is_empty() {
        return "(nobody)".to_owned();
    }
    user_ids
        .iter()
        .map(|id| mention(id))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn weekday_name(weekday: Weekday) -> &'static str {
    WEEKDAY_NAMES[weekday.num_days_from_monday() as usize]
}

/// v4 `local_day`, e.g. `Thu 03 Sep`.
pub fn local_day(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!(
        "{} {:02} {}",
        weekday_name(local.weekday()),
        local.day(),
        MONTH_NAMES[local.month0() as usize]
    )
}

/// v4 `local_time`, e.g. `21:30`.
pub fn local_time(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

pub fn hhmm(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// v4 `STATUS_LABEL`.
pub fn status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Planned => "⚠️ unconfirmed",
        RunStatus::Confirmed => "✅ confirmed",
        RunStatus::AtRisk => "❗ at risk",
        RunStatus::Otot => "🕒 own time",
        RunStatus::Done => "🏁 done",
        RunStatus::Cancelled => "🚫 cancelled",
    }
}

/// v4 `rsvp_tally`, e.g. `1/3 ✅ · 1 ❌`.
pub fn rsvp_tally(participants: &[String], rsvps: &BTreeMap<String, RsvpState>) -> String {
    let count = |state| {
        participants
            .iter()
            .filter(|uid| rsvps.get(*uid) == Some(&state))
            .count()
    };
    let (yes, no) = (count(RsvpState::Yes), count(RsvpState::No));
    let mut text = format!("{yes}/{} ✅", participants.len());
    if no > 0 {
        text.push_str(&format!(" · {no} ❌"));
    }
    text
}

/// v4 `roster_delta`: `this week: −MY +kanon`, or empty when unchanged.
pub fn roster_delta(out: &[String], joined: &[String]) -> String {
    if out.is_empty() && joined.is_empty() {
        return String::new();
    }
    let bits: Vec<String> = out
        .iter()
        .map(|name| format!("\u{2212}{name}"))
        .chain(joined.iter().map(|name| format!("+{name}")))
        .collect();
    format!("this week: {}", bits.join(" "))
}

/// v4 `schedule_line`.
pub fn schedule_line(
    run: &Run,
    zone: Tz,
    rsvps: &BTreeMap<String, RsvpState>,
    delta: &str,
) -> String {
    let when = if run.status == RunStatus::Otot {
        "own time".to_owned()
    } else {
        local_time(run.datetime, zone)
    };
    let mut parts = vec![
        format!("`#{}`", short_id(&run.id)),
        when,
        format!("**{}**", format_bosses(&run.bosses)),
        format_participants(&run.participants),
        status_label(run.status).to_owned(),
        rsvp_tally(&run.participants, rsvps),
    ];
    if !delta.is_empty() {
        parts.push(format!("_{delta}_"));
    }
    parts.join(" · ")
}

/// v4 `group_by_day`: `(day heading, runs)` in time order.
pub fn group_by_day<'a>(runs: &[&'a Run], zone: Tz) -> Vec<(String, Vec<&'a Run>)> {
    let mut sorted = runs.to_vec();
    sorted.sort_by_key(|run| run.datetime);
    let mut groups: Vec<(String, Vec<&Run>)> = Vec::new();
    for run in sorted {
        let key = local_day(run.datetime, zone);
        match groups.iter_mut().find(|(day, _)| *day == key) {
            Some((_, runs)) => runs.push(run),
            None => groups.push((key, vec![run])),
        }
    }
    groups
}

/// v4 `fixed_run_line` with the boss table (every command passes one).
pub fn fixed_run_line(fixed: &FixedRun, catalog: &BossTable) -> String {
    let place = fixed
        .channel_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .map(|id| format!(" · <#{id}>"))
        .unwrap_or_default();
    format!(
        "`#{}` **{}** · {} {} · {} · owner {}{place}\n   ↳ {}",
        short_id(&fixed.id),
        format_bosses(&fixed.bosses),
        weekday_name(fixed.weekday),
        hhmm(fixed.time),
        format_participants(&fixed.participants),
        mention(fixed.owner()),
        catalog.describe_all(&fixed.bosses),
    )
}
