//! Which run an extracted amendment is about (v4 `bot/extract/match.py`).
//!
//! Runs are matched by bosses ∩ participants, scoped to the channel the chat
//! happened in; only a channel with no live runs of its own falls back to
//! guild-wide matching, where participant overlap must carry the decision.

use std::cmp::Reverse;
use std::collections::HashSet;

use chrono::NaiveDate;
use chrono_tz::Tz;

use crate::domain::ids::{canonical, short_id};
use crate::domain::schedule::Run;
use crate::extract::{Amendment, AmendmentKind};

/// `reason_code` for "bosses were named and nothing here runs them": with a
/// day and time the caller turns it into an `add`, otherwise it is dropped.
pub const NO_BOSS_OVERLAP: &str = "no-boss-overlap";

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
/// the reset is ordinary, reaching back into an earlier week is not.
pub fn reachable<'a>(runs: &[&'a Run], day: Option<NaiveDate>, zone: Tz) -> Vec<&'a Run> {
    runs.iter()
        .copied()
        .filter(|run| !starts_after(run, day, zone))
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

/// The run `target_run_hint` points at, refused when the amendment names
/// bosses that run lacks: the model will happily point anywhere it saw a run.
fn hinted<'a>(hint: Option<&str>, runs: &[&'a Run], bosses: &HashSet<&str>) -> Option<&'a Run> {
    let prefix = canonical(hint.filter(|hint| !hint.is_empty())?);
    if prefix.chars().count() < 4 {
        return None;
    }
    let run = runs
        .iter()
        .copied()
        .find(|run| canonical(&run.id).starts_with(&prefix))?;
    (bosses.is_empty() || overlap(bosses, &run.bosses) > 0).then_some(run)
}

/// Pick the run `amendment` is about.
///
/// `guild_runs` are consulted only when the channel has no live runs. Ties are
/// broken by participant overlap with the author and anyone mentioned; a tie
/// that survives is reported `ambiguous` (first rival as the run) so the caller
/// asks instead of guessing.
pub fn match_run<'a, S: AsRef<str>>(
    amendment: &Amendment,
    channel_runs: &[&'a Run],
    guild_runs: &[&'a Run],
    author_id: Option<&str>,
    mentioned: &[S],
) -> MatchResult<'a> {
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

    if let Some(run) = hinted(amendment.target_run_hint.as_deref(), &scoped, &bosses) {
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
