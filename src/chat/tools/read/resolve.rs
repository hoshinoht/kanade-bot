//! Untrusted run and weekly-timing descriptions → one row, never a guess
//! (v4 `tools/resolution.py`).

use std::collections::BTreeSet;

use chrono::{DateTime, Datelike, NaiveDate, TimeDelta, Utc, Weekday};
use regex::Regex;

use super::ToolWorld;
use super::format::{fixed_line, local, run_line};
use crate::chat::tools::{MAX_RUNS, ToolError, ToolResult};
use crate::domain::ids::resolve_id;
use crate::domain::pytext::strip;
use crate::domain::schedule::{FixedRun, Run, RunStatus, utc_instant};
use crate::domain::weeks::materialised_week_starts;
use crate::extract::resolve::WEEKDAY_ALIASES;

/// Day words `get_run` and `get_schedule` read besides weekdays.
pub(super) const RELATIVE_DAYS: [(&str, i64); 5] = [
    ("today", 0),
    ("tonight", 0),
    ("tomorrow", 1),
    ("tmr", 1),
    ("tmrw", 1),
];

/// `word` written in `query` as a whole word.
pub(super) fn says(query: &str, word: &str) -> bool {
    Regex::new(&format!(r"\b{}\b", regex::escape(word)))
        .expect("pattern")
        .is_match(query)
}

fn days_forward(from: Weekday, to: Weekday) -> i64 {
    i64::from((7 + to.num_days_from_monday() - from.num_days_from_monday()) % 7)
}

fn referenced_dates(world: &ToolWorld<'_>, query: &str, now: DateTime<Utc>) -> BTreeSet<NaiveDate> {
    let today = local(&now, world.zone).date();
    let mut dates: BTreeSet<NaiveDate> = WEEKDAY_ALIASES
        .iter()
        .filter(|(word, _)| says(query, word))
        .map(|&(_, weekday)| today + TimeDelta::days(days_forward(today.weekday(), weekday)))
        .collect();
    dates.extend(
        RELATIVE_DAYS
            .iter()
            .filter(|(word, _)| says(query, word))
            .map(|&(_, offset)| today + TimeDelta::days(offset)),
    );
    dates
}

fn names_a_day(query: &str) -> bool {
    WEEKDAY_ALIASES.iter().any(|(word, _)| says(query, word))
        || RELATIVE_DAYS.iter().any(|(word, _)| says(query, word))
}

/// Boss keys a list of canonical tokens names, through the alias table.
fn boss_shorts(world: &ToolWorld<'_>, tokens: &[String]) -> BTreeSet<String> {
    tokens
        .iter()
        .flat_map(|token| world.catalog.names_in(token))
        .collect()
}

fn listing(world: &ToolWorld<'_>, runs: &[&Run], lead: &str, now: DateTime<Utc>) -> String {
    let lines: Vec<String> = runs
        .iter()
        .take(MAX_RUNS)
        .map(|run| run_line(world, run, false, now))
        .collect();
    format!("{lead}\n\n{}", lines.join("\n\n"))
}

fn no_run(text: &str) -> ToolError {
    ToolError(format!(
        "No run matches `{text}`. Check what is scheduled, then ask them which one they mean. Do not guess."
    ))
}

/// A run from a short id or an unambiguous boss/day description.
pub fn resolve_run<'w>(
    world: &'w ToolWorld<'_>,
    query: &str,
    now: DateTime<Utc>,
) -> ToolResult<&'w Run> {
    let text = strip(query);
    if text.is_empty() {
        return Err(ToolError::new(
            "Ask them which run they mean -- a boss and a day, like 'hstar wednesday'.",
        ));
    }
    let runs = &world.snapshot.runs;
    if let Ok(id) = resolve_id(text, runs.iter().map(|run| run.id.as_str())) {
        return Ok(runs.iter().find(|run| run.id == id).expect("resolved id"));
    }
    let low = text.to_lowercase();
    let dates = referenced_dates(world, &low, now);
    if dates.len() > 1 {
        return Err(ToolError(format!(
            "`{text}` names more than one day. Ask them which one they mean; do not guess."
        )));
    }
    let weeks = materialised_week_starts(world.zone, world.reset_weekday, world.reset_time, &now)
        .map_err(|error| ToolError(error.to_string()))?;
    let mut candidates: Vec<&Run> = Vec::new();
    for start in &weeks {
        let start = utc_instant(start).map_err(|error| ToolError(error.to_string()))?;
        candidates.extend(runs.iter().filter(|run| {
            run.week_start == start && !matches!(run.status, RunStatus::Cancelled | RunStatus::Done)
        }));
    }
    let named: BTreeSet<String> = world.catalog.names_in(&low).into_iter().collect();
    let by_boss: Vec<&Run> = candidates
        .iter()
        .copied()
        .filter(|run| !named.is_empty() && !boss_shorts(world, &run.bosses).is_disjoint(&named))
        .collect();
    let named_day = !dates.is_empty();
    if by_boss.is_empty() && !named_day {
        return Err(no_run(text));
    }
    let mut matches = if by_boss.is_empty() {
        candidates.clone()
    } else {
        by_boss.clone()
    };
    if named_day {
        let narrowed: Vec<&Run> = matches
            .iter()
            .copied()
            .filter(|run| dates.contains(&local(&run.datetime, world.zone).date()))
            .collect();
        if !narrowed.is_empty() {
            matches = narrowed;
        } else if !by_boss.is_empty() {
            let same_weekday: Vec<&Run> = by_boss
                .iter()
                .copied()
                .filter(|run| {
                    let weekday = local(&run.datetime, world.zone).weekday();
                    WEEKDAY_ALIASES
                        .iter()
                        .any(|&(word, day)| day == weekday && says(&low, word))
                })
                .collect();
            if let [only] = same_weekday.as_slice() {
                return Ok(only);
            }
            return Err(ToolError(format!(
                "No run matches `{text}`. {}",
                listing(world, &by_boss, "That boss is on", now)
            )));
        }
    }
    match matches.as_slice() {
        [] => Err(no_run(text)),
        [only] => Ok(only),
        many => Err(ToolError(format!(
            "`{text}` matches more than one run. {}",
            listing(world, many, "Ask which one:", now)
        ))),
    }
}

fn no_weekly_for(world: &ToolWorld<'_>, text: &str) -> ToolResult<()> {
    let named = world.catalog.names_in(text);
    if named.is_empty() {
        return Ok(());
    }
    let scheduled: BTreeSet<String> = world
        .snapshot
        .fixed_runs
        .iter()
        .flat_map(|fixed| boss_shorts(world, &fixed.bosses))
        .collect();
    let missing: Vec<&String> = named
        .iter()
        .filter(|short| !scheduled.contains(*short))
        .collect();
    if missing.len() != named.len() {
        return Ok(());
    }
    let label: Vec<&str> = missing
        .iter()
        .filter_map(|short| world.catalog.boss(short))
        .map(|boss| boss.full())
        .collect();
    Err(ToolError(format!(
        "No weekly timing for {} exists, so there is nothing to change. If they are asking for an existing one-off run to happen every week, that is propose_add with weekly = true -- the scheduler folds this week's run into the new weekly instead of leaving a duplicate beside it. If they meant a different boss's weekly, ask them which one; do not offer them somebody else's.",
        label.join(", ")
    )))
}

/// A weekly timing from a short id or an unambiguous boss/day description.
pub fn resolve_fixed<'w>(world: &'w ToolWorld<'_>, query: &str) -> ToolResult<&'w FixedRun> {
    let text = strip(query);
    if text.is_empty() {
        return Err(ToolError::new(
            "Ask them which weekly timing they mean -- a boss, and a day if needed.",
        ));
    }
    let all = &world.snapshot.fixed_runs;
    if let Ok(id) = resolve_id(text, all.iter().map(|fixed| fixed.id.as_str())) {
        return Ok(all
            .iter()
            .find(|fixed| fixed.id == id)
            .expect("resolved id"));
    }
    let low = text.to_lowercase();
    let named: BTreeSet<String> = world.catalog.names_in(&low).into_iter().collect();
    let by_boss: Vec<&FixedRun> = all
        .iter()
        .filter(|fixed| !named.is_empty() && !boss_shorts(world, &fixed.bosses).is_disjoint(&named))
        .collect();
    if by_boss.is_empty() {
        no_weekly_for(world, text)?;
    }
    let named_day = names_a_day(&low);
    if by_boss.is_empty() && !named_day {
        return Err(ToolError(format!(
            "No weekly timing matches `{text}`. Ask them which boss's weekly run they mean."
        )));
    }
    let mut matches: Vec<&FixedRun> = if by_boss.is_empty() {
        all.iter().collect()
    } else {
        by_boss
    };
    if named_day {
        let narrowed: Vec<&FixedRun> = matches
            .iter()
            .copied()
            .filter(|fixed| {
                WEEKDAY_ALIASES
                    .iter()
                    .any(|&(word, day)| day == fixed.weekday && says(&low, word))
            })
            .collect();
        if !narrowed.is_empty() {
            matches = narrowed;
        }
    }
    match matches.as_slice() {
        [] => Err(ToolError(format!("No weekly timing matches `{text}`."))),
        [only] => Ok(only),
        many => {
            let listed: Vec<String> = many
                .iter()
                .take(MAX_RUNS)
                .map(|fixed| fixed_line(world, fixed))
                .collect();
            Err(ToolError(format!(
                "`{text}` matches more than one weekly timing. Ask which one they mean -- name the boss and the night each one is on, and do not pick one yourself. Their answer comes back as a normal message and you can try again then, with the short id in brackets if that is clearer:\n{}",
                listed.join("\n")
            )))
        }
    }
}
