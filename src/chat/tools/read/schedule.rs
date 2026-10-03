//! `get_schedule`: one calendar or boss week, optionally narrowed by
//! channel, member or day (v4 `tools/get_schedule.py`), or the upcoming runs
//! left in this boss week for `auto` without a day (`D-AUTO-FORWARD`).

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Utc, Weekday};
use serde_json::{Map, Value};

use super::ToolWorld;
use super::format::{day_label, is_over, local, run_context, run_line};
use super::participants::py_text;
use super::resolve::RELATIVE_DAYS;
use super::roster::{bossers, resolve_participant_text};
use crate::chat::tools::{MAX_MEMBER_REPLY, MAX_RUNS, ToolContext, ToolError, ToolResult};
use crate::domain::members::member_name;
use crate::domain::pytext::strip;
use crate::domain::schedule::{Run, RunStatus, utc_instant};
use crate::domain::time::ZonedDateTime;
use crate::domain::weeks::{calendar_week_end, calendar_week_start, week_end, week_start};
use crate::extract::resolve::WEEKDAY_ALIASES;

type Interval = (ZonedDateTime, ZonedDateTime, Option<BTreeSet<NaiveDate>>);

fn failed(error: impl ToString) -> ToolError {
    ToolError(error.to_string())
}

fn utc(at: &ZonedDateTime) -> ToolResult<DateTime<Utc>> {
    utc_instant(at).map_err(failed)
}

fn shifted(at: &ZonedDateTime, days: i64) -> ToolResult<ZonedDateTime> {
    ZonedDateTime::new(at.wall() + TimeDelta::days(days), at.zone()).map_err(failed)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn elsewhere(count: usize) -> &'static str {
    if count == 1 {
        "another channel"
    } else {
        "other channels"
    }
}

/// The optional participant filter as one roster id.
fn schedule_participant(
    world: &ToolWorld<'_>,
    ctx: &ToolContext,
    args: &Map<String, Value>,
) -> ToolResult<Option<String>> {
    let Some(Value::String(value)) = args.get("participant") else {
        return Ok(ctx.self_schedule_requested.then(|| ctx.author_id.clone()));
    };
    let raw = strip(value);
    if raw.is_empty() {
        return Ok(ctx.self_schedule_requested.then(|| ctx.author_id.clone()));
    }
    if raw.to_lowercase() == "me" {
        return Ok(Some(ctx.author_id.clone()));
    }
    let bot = ctx.bot_user_id.clone().unwrap_or_default();
    let role = ctx.self_role_id.clone().unwrap_or_default();
    let mut references = Vec::new();
    if !bot.is_empty() {
        references.extend([bot.clone(), format!("<@{bot}>"), format!("<@!{bot}>")]);
    }
    if !role.is_empty() {
        references.extend([role.clone(), format!("<@&{role}>")]);
    }
    // Small models copy the bot mention into participant; read it as first person.
    if references.iter().any(|reference| reference == raw) {
        return Ok(Some(ctx.author_id.clone()));
    }
    let roster = bossers(world.members);
    let resolution = resolve_participant_text(raw, &roster);
    // A model may render the addressed bot as @name instead of <@id>.
    let named_bot = raw.strip_prefix('@').unwrap_or(raw);
    if ctx.self_schedule_requested
        && resolution.ids.is_empty()
        && resolution.ambiguous.is_empty()
        && resolution.unknown.len() == 1
        && ctx
            .bot_names
            .iter()
            .any(|name| name.to_lowercase() == named_bot.to_lowercase())
    {
        return Ok(Some(ctx.author_id.clone()));
    }
    if !resolution.unknown.is_empty() {
        return Err(ToolError(format!(
            "Nobody on the roster matches {}. Ask whose schedule they want; if they mean their own, ask them to say so.",
            resolution.unknown.join(", ")
        )));
    }
    if !resolution.ambiguous.is_empty() {
        let options: Vec<String> = resolution
            .ambiguous
            .iter()
            .map(|(name, matches)| format!("{name}: {}", matches.join(", ")))
            .collect();
        return Err(ToolError(format!(
            "Ask which person they mean -- {}.",
            options.join("; ")
        )));
    }
    match resolution.ids.as_slice() {
        [only] if world.directory.member(only).is_some() => Ok(Some(only.clone())),
        [only]
            if ctx.self_schedule_requested
                && (raw == format!("<@{only}>") || raw == format!("<@!{only}>")) =>
        {
            Ok(Some(ctx.author_id.clone()))
        }
        _ => Err(ToolError::new(
            "That does not identify one person on the roster. Ask who they mean.",
        )),
    }
}

fn dates_in_interval(
    start: &ZonedDateTime,
    end: &ZonedDateTime,
    weekday: Weekday,
) -> BTreeSet<NaiveDate> {
    let mut cursor = start.wall().date();
    let last = (end.wall() - TimeDelta::microseconds(1)).date();
    let mut dates = BTreeSet::new();
    while cursor <= last {
        if cursor.weekday() == weekday {
            dates.insert(cursor);
        }
        cursor += TimeDelta::days(1);
    }
    dates
}

fn day_bounds(world: &ToolWorld<'_>, chosen: NaiveDate) -> ToolResult<Interval> {
    let start = ZonedDateTime::new(chosen.and_time(NaiveTime::MIN), world.zone).map_err(failed)?;
    let end = shifted(&start, 1)?;
    Ok((start, end, Some(BTreeSet::from([chosen]))))
}

fn schedule_interval(
    world: &ToolWorld<'_>,
    args: &Map<String, Value>,
    (start, end): (ZonedDateTime, ZonedDateTime),
    now: DateTime<Utc>,
    week: &str,
) -> ToolResult<Interval> {
    let Some(Value::String(raw_day)) = args.get("day") else {
        return Ok((start, end, None));
    };
    let value = strip(raw_day);
    if value.is_empty() {
        return Ok((start, end, None));
    }
    let today = local(&now, world.zone).date();
    let raw = value.to_lowercase();
    if week == "next" && raw == "next" {
        return Ok((start, end, None));
    }
    if let Some(&(_, offset)) = RELATIVE_DAYS.iter().find(|(word, _)| *word == raw) {
        return day_bounds(world, today + TimeDelta::days(offset));
    }
    if let Some(&(_, weekday)) = WEEKDAY_ALIASES.iter().find(|(word, _)| *word == raw) {
        if week == "auto" {
            let ahead =
                (7 + weekday.num_days_from_monday() - today.weekday().num_days_from_monday()) % 7;
            return day_bounds(world, today + TimeDelta::days(i64::from(ahead)));
        }
        let dates = dates_in_interval(&start, &end, weekday);
        return Ok((start, end, Some(dates)));
    }
    Err(ToolError::new(
        "day must be omitted for a whole week, or be today, tonight, tomorrow, or one weekday.",
    ))
}

/// Every boss-week storage bucket overlapping the interval.
fn intersecting_buckets(
    world: &ToolWorld<'_>,
    start: &ZonedDateTime,
    end: &ZonedDateTime,
) -> ToolResult<Vec<DateTime<Utc>>> {
    let mut bucket =
        week_start(start, world.zone, world.reset_weekday, world.reset_time).map_err(failed)?;
    let end = utc(end)?;
    let mut buckets = Vec::new();
    while utc(&bucket)? < end {
        buckets.push(utc(&bucket)?);
        bucket = week_end(&bucket, world.zone).map_err(failed)?;
    }
    Ok(buckets)
}

/// Complete records inside the member reply budget.
fn bounded(heading: &str, records: &[String], footer: &str) -> String {
    let fallback =
        "**Schedule omitted**\n\n*Runs could not be displayed safely within the message limit.*";
    if records.is_empty() {
        return fallback.to_owned();
    }
    let join = |selected: &[&str], tail: &str| {
        let mut parts = vec![heading];
        parts.extend_from_slice(selected);
        parts.join("\n\n") + tail
    };
    let tail = |omitted: usize| {
        let more = if omitted > 0 {
            format!("\n\n*(and {omitted} more)*")
        } else {
            String::new()
        };
        more + footer
    };
    let mut selected: Vec<&str> = Vec::new();
    for record in records {
        let omitted = records.len() - selected.len() - 1;
        let mut trial = selected.clone();
        trial.push(record);
        let candidate = join(&trial, &tail(omitted));
        if selected.len() >= MAX_RUNS || candidate.chars().count() > MAX_MEMBER_REPLY {
            break;
        }
        selected.push(record);
    }
    if selected.is_empty() {
        return format!("{fallback}\n\n*(and {} more)*", records.len());
    }
    join(&selected, &tail(records.len() - selected.len()))
}

fn has(run: &Run, participant: &str) -> bool {
    run.participants.iter().any(|p| p == participant)
}

/// `get_schedule`.
pub fn get_schedule(
    world: &ToolWorld<'_>,
    ctx: &ToolContext,
    args: &Map<String, Value>,
) -> ToolResult<String> {
    let week = strip(&py_text(args.get("week"))).to_lowercase();
    let week = if week.is_empty() {
        "this".to_owned()
    } else {
        week
    };
    if !["this", "next", "this_boss", "next_boss", "auto"].contains(&week.as_str()) {
        return Err(ToolError::new(
            "Ask whether they mean this week, next week, or a specific day.",
        ));
    }
    let basis = if week == "this" || week == "next" {
        let basis = strip(&py_text(args.get("week_basis"))).to_lowercase();
        let basis = if basis.is_empty() {
            "calendar".to_owned()
        } else {
            basis
        };
        if basis != "calendar" && basis != "boss" {
            return Err(ToolError::new(
                "Ask whether they mean a calendar week or a boss week.",
            ));
        }
        basis
    } else if week.ends_with("_boss") {
        "boss".to_owned()
    } else {
        "calendar".to_owned()
    };
    let period_week = if week == "auto" {
        "this"
    } else {
        week.trim_end_matches("_boss")
    };
    let week_label = format!(
        "{period_week}{}",
        if basis == "boss" {
            " boss week"
        } else {
            " week"
        }
    );
    let raw_scope = strip(&py_text(args.get("scope"))).to_lowercase();
    let raw_scope = if raw_scope.is_empty() {
        "all".to_owned()
    } else {
        raw_scope
    };
    let scope = if ctx.force_channel_scope {
        "channel".to_owned()
    } else if ctx.force_all_channels {
        "all".to_owned()
    } else {
        raw_scope
    };
    if scope != "all" && scope != "channel" {
        return Err(ToolError(format!(
            "scope must be 'all' or 'channel' (got '{}'). Ask whether they want this channel or all channels.",
            match args.get("scope") {
                Some(Value::String(text)) => text.clone(),
                other => py_text(other),
            }
        )));
    }
    let participant = if ctx.force_group_schedule {
        None
    } else {
        schedule_participant(world, ctx, args)?
    };
    let for_me = participant.as_deref() == Some(ctx.author_id.as_str());
    let participant_name = participant
        .as_deref()
        .map(|uid| member_name(world.directory, uid));

    let now = ctx.now;
    let (start, end) = if basis == "calendar" {
        let mut start = calendar_week_start(&now, world.zone).map_err(failed)?;
        if period_week == "next" {
            start = shifted(&start, 7)?;
        }
        let end = calendar_week_end(&start, world.zone).map_err(failed)?;
        (start, end)
    } else {
        let current =
            week_start(&now, world.zone, world.reset_weekday, world.reset_time).map_err(failed)?;
        let start = if period_week == "next" {
            week_end(&current, world.zone).map_err(failed)?
        } else {
            current
        };
        let end = week_end(&start, world.zone).map_err(failed)?;
        (start, end)
    };
    // `auto` without a day answers "when is my next run": the upcoming runs
    // left in this boss week, across calendar weeks (D-AUTO-FORWARD).
    let forward = week == "auto"
        && !matches!(args.get("day"), Some(Value::String(day)) if !strip(day).is_empty());
    let (start, end, selected) = schedule_interval(world, args, (start, end), now, &week)?;
    let buckets = intersecting_buckets(world, &start, &end)?;
    let date_label = selected.as_ref().map(|dates| {
        dates
            .iter()
            .map(|day| day_label(*day))
            .collect::<Vec<_>>()
            .join(" or ")
    });

    let (start_utc, end_utc) = (utc(&start)?, utc(&end)?);
    let mut seen = BTreeSet::new();
    let mut everything: Vec<&Run> = Vec::new();
    if forward {
        // From the earlier of this calendar week and this boss week, so runs
        // already done this week still back the "already done" note, to the
        // end of this boss week (user decision 2026-10-03).
        let boss_week =
            week_start(&now, world.zone, world.reset_weekday, world.reset_time).map_err(failed)?;
        let since = start_utc.min(utc(&boss_week)?);
        let until = utc(&week_end(&boss_week, world.zone).map_err(failed)?)?;
        for run in &world.snapshot.runs {
            if since <= run.datetime && run.datetime < until && seen.insert(run.id.as_str()) {
                everything.push(run);
            }
        }
    } else {
        for bucket in &buckets {
            for run in world
                .snapshot
                .runs
                .iter()
                .filter(|run| run.week_start == *bucket)
            {
                if !seen.insert(run.id.as_str()) {
                    continue;
                }
                if start_utc <= run.datetime && run.datetime < end_utc {
                    everything.push(run);
                }
            }
        }
    }
    everything.sort_by(|a, b| (a.datetime, &a.id).cmp(&(b.datetime, &b.id)));
    everything.retain(|run| run.status != RunStatus::Cancelled);
    let dated: Vec<&Run> = match &selected {
        Some(dates) => everything
            .iter()
            .copied()
            .filter(|run| dates.contains(&local(&run.datetime, world.zone).date()))
            .collect(),
        None => everything,
    };

    let here = ctx.channel_id.as_str();
    let in_here = |run: &Run| run.channel_id.as_deref() == Some(here);
    let mut runs: Vec<&Run> = if scope == "channel" {
        dated.iter().copied().filter(|run| in_here(run)).collect()
    } else {
        dated.clone()
    };
    if let Some(who) = &participant {
        runs.retain(|run| has(run, who));
    }
    let matching = runs.clone();
    // A forward read lists only what is still ahead, whatever the question
    // said; a singular "next run" question gets just the soonest one.
    let upcoming_only = ctx.upcoming_only || ctx.next_only || forward;
    if upcoming_only {
        runs.retain(|run| !is_over(run, now));
    }
    if ctx.next_only {
        runs.truncate(1);
    }
    let all_over = |list: &[&Run]| list.iter().all(|run| is_over(run, now));
    let scope_label = if scope == "channel" {
        "This channel"
    } else {
        "All channels"
    };

    if runs.is_empty() {
        if upcoming_only {
            let period = match &date_label {
                Some(label) => format!(" on {label}"),
                None if forward => " this boss week".to_owned(),
                None => format!(" in {week_label}"),
            };
            if let Some(who) = &participant {
                let subject = if for_me {
                    "you".to_owned()
                } else {
                    participant_name.clone().unwrap_or_default()
                };
                let where_ = if scope == "channel" {
                    " in this channel"
                } else {
                    ""
                };
                let mut answer = format!("**No upcoming runs for {subject}{where_}{period}.**");
                let away: Vec<&&Run> = dated
                    .iter()
                    .filter(|run| !in_here(run) && has(run, who) && !is_over(run, now))
                    .collect();
                if !matching.is_empty() && all_over(&matching) {
                    answer.push_str(if for_me {
                        " Your matching scheduled runs are already done."
                    } else {
                        " Their matching scheduled runs are already done."
                    });
                }
                if !away.is_empty() && scope == "channel" {
                    let count = away.len();
                    let with_verb = if for_me {
                        "You have".to_owned()
                    } else {
                        format!("{} has", capitalize(&subject))
                    };
                    answer.push_str(&format!(
                        " {with_verb} {} in {}.",
                        plural(count, "upcoming run", "upcoming runs"),
                        elsewhere(count)
                    ));
                }
                return Ok(answer);
            }
            if scope == "channel" {
                let mut answer = format!("**No upcoming runs in this channel{period}.**");
                let away = dated
                    .iter()
                    .filter(|run| !in_here(run) && !is_over(run, now))
                    .count();
                if !matching.is_empty() && all_over(&matching) {
                    answer.push_str(" The runs scheduled here are already done.");
                }
                if away > 0 {
                    answer.push_str(&format!(
                        " The group has {} in {}.",
                        plural(away, "upcoming run", "upcoming runs"),
                        elsewhere(away)
                    ));
                }
                return Ok(answer);
            }
            if !matching.is_empty() && all_over(&matching) {
                if forward {
                    return Ok(format!(
                        "**No runs left this boss week · {scope_label}**\n\nEverything scheduled this boss week is already done."
                    ));
                }
                return Ok(format!(
                    "**No runs left {} · {scope_label}**\n\nEverything scheduled in this period is already done.",
                    date_label.as_deref().unwrap_or(&week_label)
                ));
            }
            return Ok(format!("**No upcoming runs{period} · {scope_label}.**"));
        }
        let period = match &date_label {
            Some(label) => format!("on {label}"),
            None => format!("for {week_label}"),
        };
        if let Some(who) = &participant {
            let away = if scope == "channel" {
                dated
                    .iter()
                    .filter(|run| !in_here(run) && has(run, who))
                    .count()
            } else {
                0
            };
            let subject = if for_me {
                "You are".to_owned()
            } else {
                format!("{} is", participant_name.clone().unwrap_or_default())
            };
            let also = if away > 0 {
                format!(
                    " {subject} on {} in {} {period}. If the original question did not explicitly limit the channel, check all channels before answering; otherwise ask whether they want to see those runs too.",
                    plural(away, "run", "runs"),
                    elsewhere(away)
                )
            } else {
                String::new()
            };
            let where_ = if scope == "channel" {
                " in this channel"
            } else {
                ""
            };
            return Ok(format!("{subject} not on any runs{where_} {period}.{also}"));
        }
        if scope == "channel" {
            let count = dated.len();
            let also = if count > 0 {
                format!(
                    " The group has {} in {} {}. If the original question did not explicitly limit the channel, check all channels before answering; otherwise ask whether they want to see those runs too.",
                    plural(count, "run", "runs"),
                    elsewhere(count),
                    match &date_label {
                        Some(label) => format!("on {label}"),
                        None => "this week".to_owned(),
                    }
                )
            } else {
                String::new()
            };
            return Ok(format!(
                "No runs are scheduled in this channel {period}.{also}"
            ));
        }
        return Ok(match &date_label {
            Some(label) => format!("Nothing is scheduled on {label}."),
            None => format!("Nothing is scheduled for {week_label}."),
        });
    }

    let with_channel = scope == "all";
    let records: Vec<String> = runs
        .iter()
        .map(|run| run_line(world, run, with_channel, now))
        .collect();
    let (run_count, period) = if ctx.next_only {
        // A forward read is this boss week's next run; otherwise name the period.
        let period = match (&date_label, forward) {
            (Some(label), _) => format!(" {label}"),
            (None, true) => String::new(),
            (None, false) => format!(" {week_label}"),
        };
        ("next run".to_owned(), period)
    } else if forward {
        let count = plural(runs.len(), "upcoming run", "upcoming runs");
        (count, " this boss week".to_owned())
    } else {
        let remaining = if upcoming_only { " left" } else { "" };
        let period = date_label.clone().unwrap_or_else(|| week_label.clone());
        (
            plural(runs.len(), "run", "runs"),
            format!("{remaining} {period}"),
        )
    };
    let heading = match &participant {
        Some(_) => {
            let owner = if for_me {
                "Your".to_owned()
            } else {
                format!("{}'s", participant_name.clone().unwrap_or_default())
            };
            format!("**{owner} {run_count}{period} · {scope_label}**")
        }
        None if ctx.next_only => format!("**Next run{period} · {scope_label}**"),
        None => format!("**{run_count}{period} · {scope_label}**"),
    };
    let footer = if all_over(&runs) {
        let period = match &date_label {
            Some(label) => format!("on {label}"),
            None => format!("in {week_label}"),
        };
        format!("\n\n*Every run listed has already happened — nothing upcoming is left {period}.*")
    } else {
        String::new()
    };
    let mut listing = bounded(&heading, &records, &footer);
    // D-PERSONAL-CONTEXT: added after bounding so members' records and the
    // omission count are exactly the group rendering's.
    if participant.is_some() {
        let asker = for_me.then_some(ctx.author_id.as_str());
        for (run, record) in runs.iter().zip(&records) {
            if let Some(context) = run_context(world, run, asker, now) {
                listing = listing.replacen(record.as_str(), &format!("{record}\n{context}"), 1);
            }
        }
    }
    Ok(listing)
}

/// Python `str.capitalize()`.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}
