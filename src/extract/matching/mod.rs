//! Which run an extracted amendment is about (v4 `bot/extract/match.py`).
//!
//! Runs are matched by bosses ∩ participants, scoped to the channel the chat
//! happened in; only a channel with no live runs of its own falls back to
//! guild-wide matching, where participant overlap must carry the decision.

use std::cmp::Reverse;
use std::collections::HashSet;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::domain::ids::{canonical, short_id};
use crate::domain::schedule::Run;
use crate::extract::{Amendment, AmendmentKind};

/// `reason_code` for "bosses were named and nothing here runs them": with a
/// day and time the caller turns it into an `add`, otherwise it is dropped.
pub const NO_BOSS_OVERLAP: &str = "no-boss-overlap";
/// A known terminal hint must never fall through to another run.
pub const TERMINAL_HINT: &str = "terminal-hint";

/// Kinds meaningless without a target run; `add` and `fix` create their own.
pub fn needs_run(kind: AmendmentKind) -> bool {
    !matches!(kind, AmendmentKind::Add | AmendmentKind::Fix)
}

/// True when `run`'s boss week begins after `day`, so the night `day` talks
/// about had already happened before that run's week began.
pub fn starts_after(run: &Run, day: Option<NaiveDate>, zone: Tz) -> bool {
    day.is_some_and(|day| run.week_start.with_timezone(&zone).date_naive() > day)
}

/// The runs an amendment about `day` may be about: moving a run forward past
/// the reset is ordinary, reaching back into an earlier week is not. A supplied
/// evidence week bounds only dayless RSVP/Sub answers; other callers omit it.
pub fn reachable<'a>(
    runs: &[&'a Run],
    day: Option<NaiveDate>,
    zone: Tz,
    evidence_week: Option<DateTime<Utc>>,
) -> Vec<&'a Run> {
    runs.iter()
        .copied()
        .filter(|run| match day {
            Some(_) => !starts_after(run, day, zone),
            None => evidence_week.is_none_or(|week| run.week_start == week),
        })
        .collect()
}

/// The chosen run (if any), why, and what else was in the running.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchResult<'a> {
    pub run: Option<&'a Run>,
    pub reason: String,
    pub candidates: Vec<&'a Run>,
    pub ambiguous: bool,
    /// Machine-readable `reason` for the cases callers branch on, or empty.
    pub reason_code: &'static str,
}

impl<'a> MatchResult<'a> {
    fn new(run: Option<&'a Run>, reason: impl Into<String>, candidates: Vec<&'a Run>) -> Self {
        Self {
            run,
            reason: reason.into(),
            candidates,
            ambiguous: false,
            reason_code: "",
        }
    }

    pub fn matched(&self) -> bool {
        self.run.is_some()
    }
}

fn set(items: &[String]) -> HashSet<&str> {
    items.iter().map(String::as_str).collect()
}

fn overlap(wanted: &HashSet<&str>, items: &[String]) -> usize {
    set(items)
        .iter()
        .filter(|item| wanted.contains(*item))
        .count()
}

fn live<'a>(runs: &[&'a Run]) -> Vec<&'a Run> {
    runs.iter()
        .copied()
        .filter(|run| !run.status.is_terminal())
        .collect()
}

fn hinted<'a>(hint: Option<&str>, runs: impl Iterator<Item = &'a Run>) -> Option<&'a Run> {
    let prefix = canonical(hint.filter(|hint| !hint.is_empty())?);
    if prefix.chars().count() < 4 {
        return None;
    }
    runs.into_iter()
        .find(|run| canonical(&run.id).starts_with(&prefix))
}

/// Check before reachability, live filtering or spanning: a stale hint is a
/// refusal for run-acting kinds, not permission to score a different run
/// (D-EXTRACT-STALE-HINT). A prefix also matching a live run is not stale.
pub fn refuse_terminal_hint<'a>(
    amendment: &Amendment,
    channel_runs: &[&'a Run],
    guild_runs: &[&'a Run],
) -> Option<MatchResult<'a>> {
    if !needs_run(amendment.kind) {
        return None;
    }
    let runs = channel_runs.iter().chain(guild_runs).copied();
    if hinted(
        amendment.target_run_hint.as_deref(),
        runs.clone().filter(|run| !run.status.is_terminal()),
    )
    .is_some()
    {
        return None;
    }
    let run = hinted(amendment.target_run_hint.as_deref(), runs)?;
    run.status.is_terminal().then(|| MatchResult {
        reason_code: TERMINAL_HINT,
        ..MatchResult::new(
            None,
            format!("model pointed at terminal run #{}", short_id(&run.id)),
            Vec::new(),
        )
    })
}

/// Pick the run `amendment` is about.
///
/// Terminal hints are checked across both pools first. Live `guild_runs` are
/// consulted only when the channel has no live runs. Ties are broken by
/// participant overlap with the author and anyone mentioned; a surviving tie
/// is reported `ambiguous` (first rival as the run). The planner drops it
/// except for RSVP/Sub.
pub fn match_run<'a, S: AsRef<str>>(
    amendment: &Amendment,
    channel_runs: &[&'a Run],
    guild_runs: &[&'a Run],
    author_id: Option<&str>,
    mentioned: &[S],
) -> MatchResult<'a> {
    if let Some(refusal) = refuse_terminal_hint(amendment, channel_runs, guild_runs) {
        return refusal;
    }
    let mut scoped = live(channel_runs);
    let wide = scoped.is_empty();
    if wide {
        scoped = live(guild_runs);
    }
    if scoped.is_empty() {
        return MatchResult::new(None, "no runs to match against", Vec::new());
    }

    let bosses = set(&amendment.bosses);
    let mut people = set(&amendment.participants);
    people.extend(mentioned.iter().map(AsRef::as_ref));
    people.extend(author_id.filter(|id| !id.is_empty()));

    // Live hints still require boss overlap, as in v4.
    if let Some(run) = hinted(amendment.target_run_hint.as_deref(), scoped.iter().copied())
        .filter(|run| bosses.is_empty() || overlap(&bosses, &run.bosses) > 0)
    {
        let reason = format!("model pointed at #{}", short_id(&run.id));
        return MatchResult::new(Some(run), reason, scoped);
    }

    let mut scored: Vec<((usize, usize), &'a Run)> = scoped
        .iter()
        .map(|&run| {
            let score = (
                overlap(&bosses, &run.bosses),
                overlap(&people, &run.participants),
            );
            (score, run)
        })
        .collect();
    // Stable, so equal scores keep channel order as v4's reverse sort does.
    scored.sort_by_key(|&(score, _)| Reverse(score));
    let (best_score, best) = scored[0];

    if !bosses.is_empty() && best_score.0 == 0 {
        // People alone are not enough: everyone in a party channel is on
        // everything in it.
        return MatchResult {
            reason_code: NO_BOSS_OVERLAP,
            ..MatchResult::new(None, "no run here has those bosses", scoped)
        };
    }
    if bosses.is_empty() {
        if wide && best_score.1 == 0 {
            return MatchResult::new(None, "guild-wide, and nobody named matches", Vec::new());
        }
        if scoped.len() == 1 {
            return MatchResult::new(Some(scoped[0]), "the only run in this channel", scoped);
        }
        if best_score.1 == 0 {
            return MatchResult::new(None, "no bosses named and no participant overlap", scoped);
        }
    }

    let rivals: Vec<&'a Run> = scored
        .iter()
        .filter(|(score, _)| *score == best_score)
        .map(|&(_, run)| run)
        .collect();
    if rivals.len() > 1 {
        return MatchResult {
            ambiguous: true,
            ..MatchResult::new(
                Some(rivals[0]),
                format!("{} runs match equally well", rivals.len()),
                scoped,
            )
        };
    }
    let mut reason = format!("bosses {}, participants {}", best_score.0, best_score.1);
    if wide {
        reason.push_str(" (guild-wide)");
    }
    MatchResult::new(Some(best), reason, scoped)
}

/// Every live channel run the amendment's bosses reach, in the given order;
/// only the author's own runs when they are on any.
pub fn runs_spanned<'a>(
    amendment: &Amendment,
    channel_runs: &[&'a Run],
    author_id: Option<&str>,
) -> Vec<&'a Run> {
    let wanted = set(&amendment.bosses);
    if wanted.is_empty() {
        return Vec::new();
    }
    let hits: Vec<&'a Run> = live(channel_runs)
        .into_iter()
        .filter(|run| overlap(&wanted, &run.bosses) > 0)
        .collect();
    if let Some(author) = author_id {
        let mine: Vec<&'a Run> = hits
            .iter()
            .copied()
            .filter(|run| run.participants.iter().any(|p| p == author))
            .collect();
        if !mine.is_empty() {
            return mine;
        }
    }
    hits
}
