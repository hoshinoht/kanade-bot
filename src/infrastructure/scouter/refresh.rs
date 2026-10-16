//! Which upcoming parties the background refresh looks up (pure): live
//! runs starting within [`HORIZON`] from the current and next boss weeks,
//! plus the current week's own-time runs, soonest first. A run is due when
//! a brought character's result, or the total of exactly one of its boss
//! parties' IGN sets, is missing, no longer fresh, or not about those very
//! characters. Due runs are queued while the lookups they need fit the
//! day's cap; a run whose last background lookup failed is left alone for
//! a day.

use std::{collections::HashSet, pin::Pin};

use chrono::{DateTime, TimeDelta, Utc};

use super::{
    BossParty, MAX_BATCH, Missing, RosterIgn, Subject,
    estimates::{characters, set_key},
};
use crate::domain::schedule::RunStatus;

/// How far ahead a run's party is kept fresh.
pub const HORIZON: TimeDelta = TimeDelta::days(7);

/// A run of the current or next boss week with its party resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpcomingRun {
    pub run_id: String,
    pub bosses: Vec<String>,
    pub status: RunStatus,
    pub datetime: DateTime<Utc>,
    pub week_start: DateTime<Utc>,
    pub subjects: Vec<Subject>,
    pub missing: Vec<Missing>,
    /// The distinct boss parties (see [`super::party_subjects`]); empty
    /// for one party of every subject.
    pub parties: Vec<BossParty>,
}

/// What [`UpcomingParties`] read: the current boss week's start and its
/// runs (and the next week's) in the horizon.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Upcoming {
    pub current_week: DateTime<Utc>,
    pub runs: Vec<UpcomingRun>,
}

pub type UpcomingFuture<'a> = Pin<Box<dyn Future<Output = Option<Upcoming>> + Send + 'a>>;

/// The schedule and roster the refresh reads (serve: the main store).
pub trait UpcomingParties: Send + Sync {
    /// The runs of the current and next boss weeks [`in_horizon`] at `now`,
    /// each party resolved as "Look up party" resolves it; `None` when the
    /// schedule or roster can't be read.
    fn upcoming(&self, now: DateTime<Utc>) -> UpcomingFuture<'_>;
}

/// A live run starting between `now` and [`HORIZON`] later, or a live
/// own-time run of the current week (its time is not when it is played).
pub fn in_horizon(
    status: RunStatus,
    datetime: DateTime<Utc>,
    week_start: DateTime<Utc>,
    current_week: DateTime<Utc>,
    now: DateTime<Utc>,
) -> bool {
    match status {
        RunStatus::Otot => week_start == current_week,
        status if status.is_live() => now <= datetime && datetime <= now + HORIZON,
        _ => false,
    }
}

/// A result in memory: when it was fetched and the roster characters it is
/// about.
#[derive(Clone, Copy, Debug)]
pub struct Reading<'a> {
    pub fetched_at: DateTime<Utc>,
    pub characters: &'a [String],
}

/// The estimates memory, of any age.
pub trait Memory {
    fn member(&self, ign: &RosterIgn) -> Option<Reading<'_>>;
    /// The total of exactly the IGN set `set_key`.
    fn party(&self, set_key: &str) -> Option<Reading<'_>>;
}

/// A run to look up, and how many characters it asks the sidecar for that
/// no run before it in the selection does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    pub run: UpcomingRun,
    pub lookups: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    /// Runs in the horizon.
    pub runs: usize,
    /// Of those, the due ones (backed off or past the cap included).
    pub due: usize,
    /// What to queue, soonest first.
    pub queued: Vec<Due>,
}

/// What the refresh queues from `upcoming` at `now`. `fresh` is the saved
/// `estimate_days` test, `backed_off` names a run whose last background
/// job failed, and `cap_left` is how many `ign=` lookups the cap still
/// allows. Runs with nobody to look up or a boss party of more than
/// [`MAX_BATCH`] are skipped; once a due run's lookups do not fit, nothing
/// after it is queued, so a later run never takes the budget from a sooner
/// one.
pub fn select(
    upcoming: Upcoming,
    now: DateTime<Utc>,
    fresh: impl Fn(DateTime<Utc>) -> bool,
    memory: &impl Memory,
    backed_off: impl Fn(&str) -> bool,
    mut cap_left: usize,
) -> Selection {
    let current_week = upcoming.current_week;
    let mut runs: Vec<UpcomingRun> = upcoming
        .runs
        .into_iter()
        .filter(|run| in_horizon(run.status, run.datetime, run.week_start, current_week, now))
        .collect();
    runs.sort_by(|a, b| (a.datetime, &a.run_id).cmp(&(b.datetime, &b.run_id)));
    let mut selection = Selection {
        runs: runs.len(),
        ..Selection::default()
    };
    // IGN keys a queued run already looks up: counted once.
    let mut counted: HashSet<String> = HashSet::new();
    let mut full = false;
    for run in runs {
        let whole = [BossParty::whole(run.subjects.len())];
        let parties = if run.parties.is_empty() {
            &whole[..]
        } else {
            &run.parties[..]
        };
        if run.subjects.is_empty() || parties.iter().any(|party| party.subjects.len() > MAX_BATCH) {
            continue;
        }
        let stale: Vec<String> = run
            .subjects
            .iter()
            .filter(|subject| {
                memory.member(&subject.ign).is_none_or(|held| {
                    !fresh(held.fetched_at)
                        || held.characters != std::slice::from_ref(&subject.character_id)
                })
            })
            .map(|subject| subject.ign.key())
            .collect();
        let total_due = parties.iter().any(|party| {
            let members: Vec<Subject> = party
                .subjects
                .iter()
                .filter_map(|at| run.subjects.get(*at).cloned())
                .collect();
            let ids = characters(&members);
            !members.is_empty()
                && memory
                    .party(&set_key(members.iter().map(|subject| &subject.ign)))
                    .is_none_or(|held| !fresh(held.fetched_at) || held.characters != ids.as_slice())
        });
        if stale.is_empty() && !total_due {
            continue;
        }
        selection.due += 1;
        if full || backed_off(&run.run_id) {
            continue;
        }
        let new: Vec<String> = stale
            .into_iter()
            .filter(|key| !counted.contains(key))
            .collect();
        if new.len() > cap_left {
            full = true;
            continue;
        }
        cap_left -= new.len();
        let lookups = new.len();
        counted.extend(new);
        selection.queued.push(Due { run, lookups });
    }
    selection
}

#[cfg(test)]
mod tests;
