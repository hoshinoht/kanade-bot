//! Compact schedule rendering shared by the tools (v4 `tools/rendering.py`,
//! `agent/formatting.py` labels and `api/service.py` names).

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;

use super::ToolWorld;
use crate::chat::gate::ChannelDirectory;
use crate::domain::ids::short_id;
use crate::domain::members::member_name;
use crate::domain::schedule::{FixedRun, Run, RunStatus};
use crate::domain::time::AwareDateTime;

/// v4 `DIFFICULTY_WORDS`: labels recoverable from a stored token alone.
const DIFFICULTY_WORDS: [(&str, &str); 5] = [
    ("e", "Easy"),
    ("n", "Normal"),
    ("h", "Hard"),
    ("c", "Chaos"),
    ("x", "Extreme"),
];

/// `XKalos` → `Extreme Kalos`; anything else comes back unchanged.
pub fn boss_label(token: &str) -> String {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let letter = first.to_lowercase().to_string();
    let short = chars.as_str();
    match DIFFICULTY_WORDS.iter().find(|(key, _)| *key == letter) {
        Some((_, word)) if !short.is_empty() => format!("{word} {short}"),
        _ => token.to_owned(),
    }
}

/// Every boss spelled out, `A + B`, or `(no bosses)`.
pub fn boss_labels<S: AsRef<str>>(bosses: &[S]) -> String {
    if bosses.is_empty() {
        return "(no bosses)".to_owned();
    }
    let labels: Vec<String> = bosses.iter().map(|b| boss_label(b.as_ref())).collect();
    labels.join(" + ")
}

/// v4 `WEEKDAY_NAMES`.
pub fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

/// The guild-local wall clock of an instant.
pub fn local(at: &DateTime<Utc>, zone: Tz) -> NaiveDateTime {
    at.astimezone(zone)
        .expect("stored instants are in range")
        .wall()
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `Wed 09 Sep`, as `strftime('%a %d %b')` in the C locale.
pub fn day_label(date: NaiveDate) -> String {
    format!(
        "{} {:02} {}",
        weekday_name(date.weekday()),
        date.day(),
        MONTHS[date.month0() as usize]
    )
}

/// `21:30`.
pub fn hhmm(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// `Wed 09 Sep 21:30`, as v4 `%a %d %b %H:%M`.
pub fn when_label(at: &DateTime<Utc>, zone: Tz) -> String {
    let wall = local(at, zone);
    format!("{} {}", day_label(wall.date()), hhmm(wall.time()))
}

/// A run is over when done or at/before `now`.
pub fn is_over(run: &Run, now: DateTime<Utc>) -> bool {
    run.status == RunStatus::Done || run.datetime <= now
}

/// A clickable `<#id>`, only for a channel the bot can see by name.
pub fn channel_reference(channels: &(impl ChannelDirectory + ?Sized), id: &str) -> Option<String> {
    let known = channels.channel(id)?;
    known.name.as_ref().filter(|name| !name.is_empty())?;
    let canonical: u64 = id.trim().parse().ok()?;
    Some(format!("<#{canonical}>"))
}

fn rsvps<'a>(world: &'a ToolWorld<'_>, run_id: &str) -> BTreeMap<&'a str, &'a str> {
    world
        .snapshot
        .rsvps
        .iter()
        .filter(|row| row.run_id == run_id)
        .map(|row| (row.user_id.as_str(), row.state.as_str()))
        .collect()
}

/// One two-line schedule record.
pub fn run_line(
    world: &ToolWorld<'_>,
    run: &Run,
    with_channel: bool,
    now: DateTime<Utc>,
) -> String {
    let wall = local(&run.datetime, world.zone);
    let answers = rsvps(world, &run.id);
    let yes = run
        .participants
        .iter()
        .filter(|uid| answers.get(uid.as_str()) == Some(&"yes"))
        .count();
    let mut parts = vec![
        format!("*{} · {}*", day_label(wall.date()), hhmm(wall.time())),
        format!("`{}`", run.status.as_str()),
        format!("`{yes}/{} yes`", run.participants.len()),
    ];
    if with_channel
        && let Some(where_) = run
            .channel_id
            .as_deref()
            .and_then(|id| channel_reference(world.channels, id))
    {
        parts.push(where_);
    }
    if is_over(run, now) {
        parts.push("*already happened*".to_owned());
    }
    format!(
        "`[{}]` **{}**\n{}",
        short_id(&run.id),
        boss_labels(&run.bosses),
        parts.join(" · ")
    )
}

/// Opens the model-only context line under a personal `get_schedule` record
/// (`D-PERSONAL-CONTEXT`); grounding strips it before members see anything.
pub const CONTEXT_LABEL: &str = "Context (hidden from members): ";

/// How far off a run is, in the small vocabulary grounding accepts: a day
/// phrase (`today`, `tonight` from 18:00, `tomorrow`, `in N days`) and,
/// within 12 hours, `in N minutes`/`in N hours` (rounded to the nearest).
fn time_until(run: &Run, now: DateTime<Utc>, zone: Tz) -> Vec<String> {
    let (wall, today) = (local(&run.datetime, zone), local(&now, zone).date());
    let days = (wall.date() - today).num_days();
    let day = match days {
        0 if wall.hour() >= 18 => "tonight".to_owned(),
        0 => "today".to_owned(),
        1 => "tomorrow".to_owned(),
        _ => format!("in {days} days"),
    };
    let mut phrases = vec![day];
    let minutes = (run.datetime - now).num_minutes().max(1);
    let unit =
        |count: i64, one: &str| format!("in {count} {one}{}", if count == 1 { "" } else { "s" });
    if minutes < 60 {
        phrases.push(unit(minutes, "minute"));
    } else if minutes < 12 * 60 {
        phrases.push(unit((minutes + 30) / 60, "hour"));
    }
    phrases
}

/// `A`, `A and B`, `A, B and C`.
fn names_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The model-only context for one upcoming run in a personal listing: time
/// until it from the turn clock, the asker's own answer when `asker` is set
/// (the listing is theirs), and the party members without an answer.
pub fn run_context(
    world: &ToolWorld<'_>,
    run: &Run,
    asker: Option<&str>,
    now: DateTime<Utc>,
) -> Option<String> {
    if is_over(run, now) {
        return None;
    }
    let answers = rsvps(world, &run.id);
    let mut phrases = time_until(run, now, world.zone);
    if let Some(asker) = asker {
        phrases.push(match answers.get(asker) {
            Some(state) => format!("you said {state}"),
            None => "you haven't answered".to_owned(),
        });
    }
    let waiting: Vec<String> = run
        .participants
        .iter()
        .filter(|uid| Some(uid.as_str()) != asker && !answers.contains_key(uid.as_str()))
        .map(|uid| member_name(world.directory, uid))
        .collect();
    if !waiting.is_empty() {
        phrases.push(format!("no answer yet from {}", names_list(&waiting)));
    }
    Some(format!("{CONTEXT_LABEL}{}", phrases.join(" · ")))
}

/// The full view of one run.
pub fn run_detail(world: &ToolWorld<'_>, run: &Run) -> String {
    let wall = local(&run.datetime, world.zone);
    let answers = rsvps(world, &run.id);
    let people: Vec<String> = run
        .participants
        .iter()
        .map(|uid| {
            format!(
                "{} (`{}`)",
                member_name(world.directory, uid),
                answers.get(uid.as_str()).copied().unwrap_or("no answer")
            )
        })
        .collect();
    let people = if people.is_empty() {
        "*nobody*".to_owned()
    } else {
        people.join(", ")
    };
    format!(
        "**Run `[{}]`**\n\n**{}** · *{} {}* · `{}`\nOn it: {people}.",
        short_id(&run.id),
        boss_labels(&run.bosses),
        day_label(wall.date()),
        hhmm(wall.time()),
        run.status.as_str()
    )
}

/// `Wed 21:30`: the weekly day and time that identify a timing.
pub fn fixed_when(fixed: &FixedRun) -> String {
    format!("{} {}", weekday_name(fixed.weekday), hhmm(fixed.time))
}

/// One weekly timing with enough on it to tell it from another.
pub fn fixed_line(world: &ToolWorld<'_>, fixed: &FixedRun) -> String {
    let party: Vec<String> = fixed
        .participants
        .iter()
        .map(|uid| member_name(world.directory, uid))
        .collect();
    let mut line = format!(
        "[{}] every {} {}",
        short_id(&fixed.id),
        fixed_when(fixed),
        boss_labels(&fixed.bosses)
    );
    if !party.is_empty() {
        line.push_str(&format!(" with {}", party.join(", ")));
    }
    line
}

/// Monday-zero weekday index.
pub fn weekday_index(weekday: Weekday) -> u32 {
    weekday.num_days_from_monday()
}

/// The local date's weekday of an instant.
pub fn local_weekday(at: &DateTime<Utc>, zone: Tz) -> Weekday {
    local(at, zone).weekday()
}
