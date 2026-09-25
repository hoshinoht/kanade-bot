use std::collections::HashMap;

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Utc};
use chrono_tz::Tz;

use super::{Payload, Planned};
use crate::domain::schedule::{RUN_DONE_AFTER, Run, RunStatus};
use crate::domain::time::{DateOutOfRange, ZonedDateTime};
use crate::extract::resolve::Resolved;
use crate::extract::{Amendment, AmendmentKind};

/// How far into the past a proposed time may point: live chat routinely
/// settles a run just after it was due ("start now lah"), a rescan does not.
pub const STALE_GRACE: TimeDelta = TimeDelta::hours(3);

/// Timing kinds that collide on one run, weakest first; `move-with-day` is a
/// `move` that names a night. `sub`, `add`, `fix` and `split` never collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Precedence {
    Move,
    Otot,
    MoveWithDay,
    Cancel,
}

fn precedence(entry: &Planned<'_>) -> Option<Precedence> {
    match entry.kind() {
        AmendmentKind::Move if entry.resolved.day.is_some() => Some(Precedence::MoveWithDay),
        AmendmentKind::Move => Some(Precedence::Move),
        AmendmentKind::Otot => Some(Precedence::Otot),
        AmendmentKind::Cancel => Some(Precedence::Cancel),
        _ => None,
    }
}

/// Settled beats asked whatever the kinds, then precedence.
fn claim(entry: &Planned<'_>) -> (bool, Option<Precedence>) {
    (!entry.amendment.is_question, precedence(entry))
}

/// Keep one timing change per run; the loser's kind is noted on the winner.
/// Ties go to the later entry: saying the same kind of thing twice is the
/// thread changing its mind.
pub fn one_per_run<'a>(entries: Vec<Planned<'a>>) -> Vec<Planned<'a>> {
    let mut best: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Planned<'a>> = Vec::new();
    for mut entry in entries {
        let (Some(run), Some(_)) = (entry.run, precedence(&entry)) else {
            out.push(entry);
            continue;
        };
        let Some(&held) = best.get(&run.id) else {
            best.insert(run.id.clone(), out.len());
            out.push(entry);
            continue;
        };
        if claim(&entry) >= claim(&out[held]) {
            let loser = out[held].kind();
            if !entry.also_mentioned.contains(&loser) {
                entry.also_mentioned.push(loser);
            }
            out[held] = entry;
        } else {
            let loser = entry.kind();
            let winner = &mut out[held];
            if !winner.also_mentioned.contains(&loser) {
                winner.also_mentioned.push(loser);
            }
        }
    }
    out
}

#[derive(PartialEq, Eq)]
enum Target {
    Run(String),
    Bosses(Vec<String>),
}

/// One entry per thing changed across a rescan's bursts, the latest winning
/// in the first one's place; keyed on the run, or the boss set an `add`/`fix`
/// would create.
pub fn consolidate<'a>(entries: Vec<Planned<'a>>) -> Vec<Planned<'a>> {
    let mut best: Vec<((AmendmentKind, Target), Planned<'a>)> = Vec::new();
    for entry in entries {
        let target = match entry.run {
            Some(run) => Target::Run(run.id.clone()),
            None => {
                let mut bosses = entry.amendment.bosses.clone();
                bosses.sort();
                Target::Bosses(bosses)
            }
        };
        let key = (entry.kind(), target);
        match best.iter_mut().find(|(known, _)| *known == key) {
            Some((_, slot)) => *slot = entry,
            None => best.push((key, entry)),
        }
    }
    one_per_run(best.into_iter().map(|(_, entry)| entry).collect())
}

fn at(
    day: NaiveDate,
    clock: NaiveTime,
    zone: Tz,
    assumed_pm: bool,
) -> Result<Resolved, DateOutOfRange> {
    Ok(Resolved {
        day: Some(day),
        clock: Some(clock),
        at: Some(ZonedDateTime::new(day.and_time(clock), zone)?),
        assumed_pm,
    })
}

/// Fill a `move`'s unsaid half from the run it moves: a day-only move keeps
/// the run's time, a time-only move keeps the run's day.
///
/// # Errors
/// [`DateOutOfRange`] outside years 1..=9999.
pub fn inherit_from_run(entry: &Planned<'_>, zone: Tz) -> Result<Resolved, DateOutOfRange> {
    let resolved = entry.resolved;
    let Some(run) = entry.run.filter(|_| entry.kind() == AmendmentKind::Move) else {
        return Ok(resolved);
    };
    let local = run.datetime.with_timezone(&zone).naive_local();
    match (resolved.day, resolved.clock) {
        (Some(day), None) => at(day, local.time(), zone, resolved.assumed_pm),
        // Once a clock parsed the day is always set; the empty `day_ref` is
        // what says no day was given.
        (_, Some(clock))
            if entry
                .amendment
                .day_ref
                .as_deref()
                .unwrap_or_default()
                .is_empty() =>
        {
            at(local.date(), clock, zone, resolved.assumed_pm)
        }
        _ => Ok(resolved),
    }
}

/// Applying this would change nothing: the run already has that slot or state.
pub fn is_no_op(entry: &Planned<'_>, zone: Tz) -> bool {
    let Some(run) = entry.run else {
        return false;
    };
    match entry.kind() {
        // An `add` that matched an existing run proposes a night already on.
        AmendmentKind::Add => true,
        AmendmentKind::Move => match (entry.resolved.at, entry.resolved.day) {
            (Some(at), _) => at.to_fixed() == run.datetime,
            (None, Some(day)) => day == run.datetime.with_timezone(&zone).date_naive(),
            (None, None) => false,
        },
        AmendmentKind::Otot => run.status == RunStatus::Otot,
        AmendmentKind::Cancel => run.status == RunStatus::Cancelled,
        _ => false,
    }
}

/// Acting on this would change something already over: a day before today,
/// a time more than [`STALE_GRACE`] behind `now`, or a finished run.
pub fn already_passed(entry: &Planned<'_>, now: DateTime<Utc>, zone: Tz) -> bool {
    if entry
        .resolved
        .at
        .is_some_and(|at| at.to_fixed() < now - STALE_GRACE)
    {
        return true;
    }
    if entry
        .resolved
        .day
        .is_some_and(|day| day < now.with_timezone(&zone).date_naive())
    {
        return true;
    }
    entry
        .run
        .is_some_and(|run| run.status.is_terminal() || run.datetime + RUN_DONE_AFTER < now)
}

/// Who offered to stand in: authors of a `sub`'s evidence other than the
/// people leaving.
pub fn volunteers_for(amendment: &Amendment, author_ids: &HashMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for message_id in &amendment.evidence_message_ids {
        if let Some(author) = author_ids.get(message_id)
            && !author.is_empty()
            && !amendment.participants.contains(author)
            && !out.contains(author)
        {
            out.push(author.clone());
        }
    }
    out
}

pub(super) fn payload_for(
    amendment: &Amendment,
    resolved: &Resolved,
    run: Option<&Run>,
    volunteers: &[String],
) -> Payload {
    match amendment.kind {
        AmendmentKind::Fix => match (resolved.day, resolved.clock) {
            (Some(day), Some(time)) => Payload::Fix {
                weekday: day.weekday(),
                time,
            },
            _ => Payload::Empty,
        },
        AmendmentKind::Split => match run {
            Some(run) => {
                let moved: Vec<String> = amendment
                    .bosses
                    .iter()
                    .filter(|boss| run.bosses.contains(boss))
                    .cloned()
                    .collect();
                Payload::Split {
                    bosses: if moved.is_empty() {
                        amendment.bosses.clone()
                    } else {
                        moved
                    },
                    participants: amendment.participants.clone(),
                }
            }
            None => Payload::Empty,
        },
        // Whoever asks for a temp is leaving; anyone else cited is offering.
        AmendmentKind::Sub => Payload::Sub {
            remove: amendment.participants.clone(),
            add: volunteers
                .iter()
                .filter(|id| !amendment.participants.contains(id))
                .cloned()
                .collect(),
        },
        _ => Payload::Empty,
    }
}
