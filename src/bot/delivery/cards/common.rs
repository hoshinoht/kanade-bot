//! v4 `bot.agent.formatting` helpers shared by the reminder cards.

use std::collections::BTreeMap;

use chrono_tz::Tz;

use crate::domain::attendance::{AnswerState, snapshot_states};
use crate::domain::catalog::BossTable;
use crate::domain::members::Directory;
use crate::domain::notify::display_names;
use crate::domain::schedule::{Run, ScheduleSnapshot};

use super::redesign::{DifficultyMarks, V2Kit};

/// v4 `REACT_HINT`.
pub const REACT_HINT: &str = "React \u{2705} if you're on, \u{274c} if not.";

/// What a card is rendered from besides its intent.
#[derive(Clone, Copy)]
pub struct CardContext<'a> {
    pub schedule: &'a ScheduleSnapshot,
    pub zone: Tz,
    pub quiet: bool,
    pub members: &'a (dyn Directory + Sync),
    /// Boss detail lines, colours and art; `None` renders v4's no-table forms.
    pub catalog: Option<&'a BossTable>,
    /// Difficulty emojis for redesigned boss labels.
    pub marks: &'a DifficultyMarks,
    /// Where a redesigned card that pings nobody (the digest) may also be
    /// laid out as Components V2; `None` keeps the embed (the admin
    /// preview, header pre-generation).
    pub v2: Option<&'a V2Kit>,
}

impl CardContext<'_> {
    pub fn run(&self, id: &str) -> Option<&Run> {
        self.schedule.runs.iter().find(|run| run.id == id)
    }

    pub fn states(&self, run: &Run) -> Vec<(String, AnswerState)> {
        snapshot_states(self.schedule, run)
    }
}

/// The runs a day-of card names, in time order (missing runs skipped).
pub fn card_runs<'a>(ctx: &CardContext<'a>, run_ids: &[String]) -> Vec<&'a Run> {
    let mut runs: Vec<&Run> = run_ids
        .iter()
        .filter_map(|id| ctx.schedule.runs.iter().find(|run| &run.id == id))
        .collect();
    runs.sort_by_key(|run| run.datetime);
    runs
}

/// v4 `lead_colour`: the first boss's catalog colour, else `default`.
pub fn lead_colour(bosses: &[String], catalog: Option<&BossTable>, default: u32) -> u32 {
    catalog
        .zip(bosses.first())
        .and_then(|(table, lead)| table.split(lead))
        .and_then(|(_, boss)| boss.guide_colour())
        .unwrap_or(default)
}

/// Participants whose answer is unknown (v4 `unanswered`: no ✅/❌; in v5
/// an assumed answer counts as answered).
pub fn unanswered(states: &[(String, AnswerState)]) -> Vec<String> {
    pick(states, |state| *state == AnswerState::Unknown)
}

/// v4 `declined`.
pub fn declined(states: &[(String, AnswerState)]) -> Vec<String> {
    pick(states, |state| *state == AnswerState::Declined)
}

/// v4 `not_declined`.
pub fn not_declined(states: &[(String, AnswerState)]) -> Vec<String> {
    pick(states, |state| *state != AnswerState::Declined)
}

fn pick(states: &[(String, AnswerState)], keep: impl Fn(&AnswerState) -> bool) -> Vec<String> {
    states
        .iter()
        .filter(|(_, state)| keep(state))
        .map(|(user, _)| user.clone())
        .collect()
}

/// v4 `everyone_on`: each participant once, in run order.
pub fn everyone_on(runs: &[&Run]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for user in runs.iter().flat_map(|run| &run.participants) {
        if !seen.contains(user) {
            seen.push(user.clone());
        }
    }
    seen
}

/// v4 `Audience`: the allow-listed are `<@id>`, everyone else their name.
/// Quiet mode never renders a mention tag, even for a nameless member.
pub struct People {
    names: BTreeMap<String, String>,
    mentioned: Vec<String>,
    quiet: bool,
}

/// How a member with no known name reads while quiet.
pub const UNNAMED: &str = "(unnamed)";

impl People {
    pub fn new(ctx: &CardContext<'_>, listed: &[String], mentioned: &[String]) -> Self {
        Self {
            names: display_names(ctx.members, listed).into_iter().collect(),
            mentioned: if ctx.quiet {
                Vec::new()
            } else {
                mentioned.to_vec()
            },
            quiet: ctx.quiet,
        }
    }

    fn one(&self, user: &str) -> String {
        if self.mentioned.iter().any(|id| id == user) {
            return format!("<@{user}>");
        }
        match self.names.get(user) {
            Some(name) if !name.is_empty() => name.clone(),
            _ if self.quiet => UNNAMED.to_owned(),
            _ => format!("<@{user}>"),
        }
    }

    /// v4 `format_participants`.
    pub fn list(&self, users: &[String]) -> String {
        if users.is_empty() {
            return "(nobody)".to_owned();
        }
        self.each(users).join(" ")
    }

    /// Each user as [`Self::list`] renders them.
    pub fn each(&self, users: &[String]) -> Vec<String> {
        users.iter().map(|user| self.one(user)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::schedule::{EMOJI_NO, EMOJI_YES};

    #[test]
    fn v4_constants() {
        assert_eq!(
            REACT_HINT,
            format!("React {EMOJI_YES} if you're on, {EMOJI_NO} if not.")
        );
        assert_eq!(
            crate::bot::commands::text::format_bosses(&[]),
            "(no bosses)"
        );
    }
}
