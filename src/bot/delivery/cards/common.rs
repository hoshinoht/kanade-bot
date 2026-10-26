//! v4 `bot.agent.formatting` helpers shared by the reminder cards.

use std::collections::BTreeMap;

use chrono_tz::Tz;

use crate::bot::commands::text;
use crate::domain::attendance::{AnswerState, Tally, snapshot_states, status_label};
use crate::domain::catalog::BossTable;
use crate::domain::members::Directory;
use crate::domain::notify::display_names;
use crate::domain::schedule::{Run, ScheduleSnapshot};
use crate::domain::settings::MessageStyle;

use super::redesign::{DifficultyMarks, V2Kit};

/// v4 `REACT_HINT`.
pub const REACT_HINT: &str = "React \u{2705} if you're on, \u{274c} if not.";
/// v4 `COLOUR_DAY_OF` (blurple).
pub const COLOUR_DAY_OF: u32 = 0x5865F2;
/// v4 `COLOUR_COUNTDOWN` (yellow).
pub const COLOUR_COUNTDOWN: u32 = 0xFEE75C;
/// v4 `COLOUR_ALL_SET` (green).
pub const COLOUR_ALL_SET: u32 = 0x57F287;
/// v4 `COLOUR_DIGEST`.
pub const COLOUR_DIGEST: u32 = 0x5865F2;

/// What a card is rendered from besides its intent.
#[derive(Clone, Copy)]
pub struct CardContext<'a> {
    pub schedule: &'a ScheduleSnapshot,
    pub zone: Tz,
    pub quiet: bool,
    pub members: &'a (dyn Directory + Sync),
    /// Boss detail lines, colours and art; `None` renders v4's no-table forms.
    pub catalog: Option<&'a BossTable>,
    /// Presentation only; the attendance policy still decides tallies and pings.
    pub style: MessageStyle,
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

/// v4 `format_offset`: `60` → `1h`, `90` → `1h30m`, `15` → `15m`.
pub fn format_offset(minutes: i64) -> String {
    let (hours, mins) = (minutes.div_euclid(60), minutes.rem_euclid(60));
    match (hours, mins) {
        (0, mins) => format!("{mins}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, mins) => format!("{hours}h{mins}m"),
    }
}

/// v4 `boss_detail`: one line per boss with its catalog description.
pub fn boss_detail(bosses: &[String], catalog: Option<&BossTable>) -> String {
    match catalog {
        None => format!("**{}**", text::format_bosses(bosses)),
        Some(table) => bosses
            .iter()
            .map(|name| format!("**{name}** · {}", table.describe(name)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// v4 `lead_colour`: the first boss's catalog colour, else `default`.
pub fn lead_colour(bosses: &[String], catalog: Option<&BossTable>, default: u32) -> u32 {
    catalog
        .zip(bosses.first())
        .and_then(|(table, lead)| table.split(lead))
        .and_then(|(_, boss)| boss.guide_colour())
        .unwrap_or(default)
}

/// `4/4 (2 assumed)`, then the status label: `, expected` when a
/// confirmation rests on assumed answers, `, confirmed (set by admin)` for
/// a hand-set status.
pub fn tally_text(schedule: &ScheduleSnapshot, run: &Run) -> String {
    let states = snapshot_states(schedule, run);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    match status_label(run.status, run.status_pin, &tally) {
        Some(label) => format!("{tally}, {label}"),
        None => tally.to_string(),
    }
}

/// A run's answers as a card shows them: the tally (assumed answers,
/// `expected`).
pub fn answers_text(ctx: &CardContext<'_>, run: &Run) -> String {
    tally_text(ctx.schedule, run)
}

/// v4 `status_line`.
pub fn status_line(ctx: &CardContext<'_>, run: &Run) -> String {
    format!(
        "{} · {}",
        text::status_label(run.status),
        answers_text(ctx, run)
    )
}

/// Participants whose answer is unknown (v4 `unanswered`: no ✅/❌; an
/// assumed answer counts as answered).
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
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::domain::attendance::StatusPin;
    use crate::domain::schedule::{EMOJI_NO, EMOJI_YES, RunSource, RunStatus};

    fn schedule(pin: Option<StatusPin>) -> ScheduleSnapshot {
        let at = Utc.with_ymd_and_hms(2026, 9, 12, 13, 0, 0).unwrap();
        ScheduleSnapshot {
            runs: vec![Run {
                id: "r".into(),
                fixed_run_id: None,
                channel_id: None,
                week_start: at,
                datetime: at,
                bosses: vec!["Kalos".into()],
                participants: vec!["1".into(), "2".into()],
                status: RunStatus::Confirmed,
                source: RunSource::Amend,
                attendance: Vec::new(),
                status_pin: pin,
            }],
            ..ScheduleSnapshot::default()
        }
    }

    #[test]
    fn a_pinned_status_reads_set_by_admin() {
        let pin = Some(StatusPin {
            status: RunStatus::Confirmed,
            at: Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap(),
        });
        let pinned = schedule(pin);
        assert_eq!(
            tally_text(&pinned, &pinned.runs[0]),
            "0/2, confirmed (set by admin)"
        );
        let plain = schedule(None);
        assert_eq!(tally_text(&plain, &plain.runs[0]), "0/2");
    }

    #[test]
    fn v4_constants_and_offsets() {
        assert_eq!(
            REACT_HINT,
            format!("React {EMOJI_YES} if you're on, {EMOJI_NO} if not.")
        );
        assert_eq!(format_offset(60), "1h");
        assert_eq!(format_offset(90), "1h30m");
        assert_eq!(format_offset(15), "15m");
        assert_eq!(format_offset(0), "0m");
        assert_eq!(text::format_bosses(&[]), "(no bosses)");
    }
}
