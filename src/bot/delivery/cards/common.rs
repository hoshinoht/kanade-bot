//! v4 `bot.agent.formatting` helpers shared by the reminder cards.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Timelike, Utc};
use chrono_tz::Tz;

use crate::domain::attendance::{
    AnswerState, AttendanceMode, AttendancePolicy, Tally, snapshot_states, status_label,
};
use crate::domain::catalog::BossTable;
use crate::domain::members::Directory;
use crate::domain::notify::display_names;
use crate::domain::schedule::{Run, RunStatus, ScheduleSnapshot};

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

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// What a card is rendered from besides its intent.
#[derive(Clone, Copy)]
pub struct CardContext<'a> {
    pub schedule: &'a ScheduleSnapshot,
    pub attendance: AttendancePolicy,
    pub zone: Tz,
    pub quiet: bool,
    pub members: &'a (dyn Directory + Sync),
    /// Boss detail lines, colours and art; `None` renders v4's no-table forms.
    pub catalog: Option<&'a BossTable>,
}

impl CardContext<'_> {
    pub fn run(&self, id: &str) -> Option<&Run> {
        self.schedule.runs.iter().find(|run| run.id == id)
    }

    pub fn states(&self, run: &Run) -> Vec<(String, AnswerState)> {
        snapshot_states(self.schedule, run, self.attendance.mode)
    }
}

/// v4 `STATUS_LABEL`.
pub fn status_text(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Planned => "⚠️ unconfirmed",
        RunStatus::Confirmed => "✅ confirmed",
        RunStatus::AtRisk => "❗ at risk",
        RunStatus::Otot => "🕒 own time",
        RunStatus::Done => "🏁 done",
        RunStatus::Cancelled => "🚫 cancelled",
    }
}

/// v4 `format_bosses`: the stored tokens joined with ` + `.
pub fn format_bosses(bosses: &[String]) -> String {
    if bosses.is_empty() {
        "(no bosses)".to_owned()
    } else {
        bosses.join(" + ")
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

/// v4 `local_time`: `HH:MM` in the guild zone.
pub fn local_time(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

/// v4 `local_day`: `Fri 25 Sep` in the guild zone.
pub fn local_day(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!(
        "{} {:02} {}",
        WEEKDAY_NAMES[local.weekday().num_days_from_monday() as usize],
        local.day(),
        MONTH_NAMES[local.month0() as usize]
    )
}

/// v4 `boss_detail`: one line per boss with its catalog description.
pub fn boss_detail(bosses: &[String], catalog: Option<&BossTable>) -> String {
    match catalog {
        None => format!("**{}**", format_bosses(bosses)),
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

/// v4 `rsvp_tally`: explicit ✅ over the party, then the ❌ count if any.
fn rsvp_tally(states: &[(String, AnswerState)]) -> String {
    let yes = count(states, |state| *state == AnswerState::Confirmed);
    let no = count(states, |state| *state == AnswerState::Declined);
    let mut text = format!("{yes}/{} \u{2705}", states.len());
    if no > 0 {
        text.push_str(&format!(" · {no} \u{274c}"));
    }
    text
}

fn count(states: &[(String, AnswerState)], keep: impl Fn(&AnswerState) -> bool) -> usize {
    states.iter().filter(|(_, state)| keep(state)).count()
}

/// `4/4 (2 assumed)`, then the status label: `, expected` when a
/// confirmation rests on assumed answers, `, confirmed (set by admin)` for
/// a hand-set status (v5 only).
pub fn tally_text(schedule: &ScheduleSnapshot, run: &Run, attendance: AttendancePolicy) -> String {
    let states = snapshot_states(schedule, run, attendance.mode);
    let tally = Tally::of(states.iter().map(|(_, state)| state));
    match status_label(run.status, run.status_pin, &tally, attendance.mode) {
        Some(label) => format!("{tally}, {label}"),
        None => tally.to_string(),
    }
}

/// A run's answers as a card shows them: v4's `rsvp_tally` in v4-compat
/// mode, the v5 tally (assumed answers, `expected`) otherwise.
pub fn answers_text(ctx: &CardContext<'_>, run: &Run) -> String {
    match ctx.attendance.mode {
        AttendanceMode::V4Compat => rsvp_tally(&ctx.states(run)),
        AttendanceMode::V5 => tally_text(ctx.schedule, run, ctx.attendance),
    }
}

/// v4 `status_line`.
pub fn status_line(ctx: &CardContext<'_>, run: &Run) -> String {
    format!("{} · {}", status_text(run.status), answers_text(ctx, run))
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
        users
            .iter()
            .map(|user| self.one(user))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::attendance::StatusPin;
    use crate::domain::schedule::{EMOJI_NO, EMOJI_YES, RunSource};

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
    fn a_pinned_status_reads_set_by_admin_in_v5_only() {
        let pin = Some(StatusPin {
            status: RunStatus::Confirmed,
            at: Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap(),
        });
        let pinned = schedule(pin);
        assert_eq!(
            tally_text(&pinned, &pinned.runs[0], AttendancePolicy::V5),
            "0/2, confirmed (set by admin)"
        );
        let plain = schedule(None);
        assert_eq!(
            tally_text(&plain, &plain.runs[0], AttendancePolicy::V5),
            "0/2"
        );
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
        assert_eq!(format_bosses(&[]), "(no bosses)");
    }
}
