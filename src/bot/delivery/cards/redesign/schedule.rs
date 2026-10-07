//! The redesigned `/schedule` reply: one ink-blue embed titled with the
//! scope (the channel itself, your runs or every run), a subtext summary
//! of the boss week, then one field per day in the digest's run shape: a
//! headline (state, guild-zone time, bosses) and a subtext line (the party
//! by answer, a one-week stand-in, the run id). Names, never tags: the
//! reply pings nobody.

use chrono::{DateTime, Datelike, TimeDelta, Utc};
use chrono_tz::Tz;
use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::{EmbedField, EmbedFooter};

use crate::domain::attendance::{AnswerState, AttendanceMode, snapshot_states};
use crate::domain::catalog::BossTable;
use crate::domain::ids::short_id;
use crate::domain::schedule::{Run, RunStatus, ScheduleSnapshot};

use super::super::common::{local_day, local_time};
use super::vocab::{DifficultyMarks, INK_BLUE, boss_labels, subtext};

/// The footer with runs hidden, and without.
pub const SCHEDULE_FOOTER_HIDDEN: &str =
    "show_past:True for cleared runs · /amend to move · /swap for a stand-in";
pub const SCHEDULE_FOOTER: &str = "/amend to move · /swap for a stand-in";

/// Whose runs the reply shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleScope<'a> {
    /// The channel it was asked in, by name when known.
    Channel(Option<&'a str>),
    Mine,
    All,
}

/// One `/schedule` answer before rendering.
pub struct ScheduleWeek<'a> {
    pub snapshot: &'a ScheduleSnapshot,
    /// Every run in scope this boss week, past and cancelled included.
    pub runs: &'a [&'a Run],
    pub show_past: bool,
    pub week_start: DateTime<Utc>,
    pub next: bool,
    pub scope: ScheduleScope<'a>,
    pub zone: Tz,
    pub attendance: AttendanceMode,
    pub catalog: Option<&'a BossTable>,
    pub marks: &'a DifficultyMarks,
    /// Said when nothing in scope is left to show.
    pub empty: &'a str,
}

fn emoji(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Done => "🏁",
        RunStatus::Confirmed => "✅",
        RunStatus::Planned => "⚠️",
        RunStatus::AtRisk => "❗",
        RunStatus::Otot => "🕒",
        RunStatus::Cancelled => "🚫",
    }
}

fn title(scope: ScheduleScope<'_>, next: bool) -> String {
    let (cap, low) = if next {
        ("Next", "next")
    } else {
        ("This", "this")
    };
    match scope {
        ScheduleScope::Channel(Some(name)) => format!("{cap} boss week in #{name}"),
        ScheduleScope::Channel(None) => format!("{cap} boss week in this channel"),
        ScheduleScope::Mine => format!("Your runs {low} boss week"),
        ScheduleScope::All => format!("Every run {low} boss week"),
    }
}

/// `Thu 03 → Wed 09 Sep`, the month written once when both days share it.
fn range(week_start: DateTime<Utc>, zone: Tz) -> String {
    let last = week_start + TimeDelta::days(7) - TimeDelta::seconds(1);
    let first_day = local_day(week_start, zone);
    let last_day = local_day(last, zone);
    let same_month = week_start.with_timezone(&zone).month() == last.with_timezone(&zone).month();
    match first_day.rsplit_once(' ') {
        Some((without_month, _)) if same_month => format!("{without_month} → {last_day}"),
        _ => format!("{first_day} → {last_day}"),
    }
}

/// `Thu 03 → Wed 09 Sep · 3 to go · 2 cleared (hidden)`.
fn summary(week: &ScheduleWeek<'_>) -> String {
    let count = |status: RunStatus| week.runs.iter().filter(|run| run.status == status).count();
    let live = week.runs.iter().filter(|run| run.status.is_live()).count();
    let hidden = if week.show_past { "" } else { " (hidden)" };
    let mut parts = vec![range(week.week_start, week.zone), format!("{live} to go")];
    for (status, word) in [
        (RunStatus::Done, "cleared"),
        (RunStatus::Cancelled, "cancelled"),
    ] {
        let n = count(status);
        if n > 0 {
            parts.push(format!("{n} {word}{hidden}"));
        }
    }
    parts.join(" · ")
}

/// This week's line-up against its weekly timing: `kanon standing in for
/// Alvin`, or empty when unchanged (or a one-off).
fn stand_in(week: &ScheduleWeek<'_>, run: &Run, name: &dyn Fn(&str) -> String) -> String {
    let Some(fixed) = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| week.snapshot.fixed_runs.iter().find(|fixed| fixed.id == id))
    else {
        return String::new();
    };
    let list = |from: &[String], without: &[String]| -> String {
        from.iter()
            .filter(|user| !without.contains(user))
            .map(|user| name(user))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let out = list(&fixed.participants, &run.participants);
    let joined = list(&run.participants, &fixed.participants);
    match (joined.is_empty(), out.is_empty()) {
        (false, false) => format!("{joined} standing in for {out}"),
        (false, true) => format!("{joined} joining this week"),
        (true, false) => format!("{out} sitting out this week"),
        (true, true) => String::new(),
    }
}

/// The two lines of one run.
fn run_lines(week: &ScheduleWeek<'_>, run: &Run, name: &dyn Fn(&str) -> String) -> String {
    let when = if run.status == RunStatus::Otot {
        "own time".to_owned()
    } else {
        format!("`{}`", local_time(run.datetime, week.zone))
    };
    let (mut in_, mut waiting, mut out) = (Vec::new(), Vec::new(), Vec::new());
    for (user, state) in snapshot_states(week.snapshot, run, week.attendance) {
        let who = name(&user);
        match state {
            AnswerState::Confirmed => in_.push(who),
            AnswerState::Assumed(_) => in_.push(format!("{who} (assumed)")),
            AnswerState::Unknown => waiting.push(who),
            AnswerState::Declined => out.push(who),
        }
    }
    let mut parts: Vec<String> = [("In", in_), ("Waiting", waiting), ("Out", out)]
        .into_iter()
        .filter(|(_, people)| !people.is_empty())
        .map(|(word, people)| format!("{word} {}", people.join(", ")))
        .collect();
    let stand_in = stand_in(week, run, name);
    if !stand_in.is_empty() {
        parts.push(stand_in);
    }
    parts.push(format!("#{}", short_id(&run.id)));
    format!(
        "{} {when}  {}\n{}",
        emoji(run.status),
        boss_labels(&run.bosses, week.catalog, week.marks),
        subtext(&format!("  {}", parts.join(" · ")))
    )
}

/// The reply's embed; `name` is how a member is written.
pub fn schedule_embed(week: &ScheduleWeek<'_>, name: &dyn Fn(&str) -> String) -> Embed {
    let mut shown: Vec<&Run> = week
        .runs
        .iter()
        .copied()
        .filter(|run| week.show_past || run.status.is_live())
        .collect();
    shown.sort_by_key(|run| run.datetime);
    let hidden = week.runs.len() - shown.len();
    let mut description = subtext(&summary(week));
    let mut fields: Vec<EmbedField> = Vec::new();
    for run in &shown {
        let day = local_day(run.datetime, week.zone);
        let lines = run_lines(week, run, name);
        match fields.last_mut() {
            Some(field) if field.name == day => {
                field.value.push('\n');
                field.value.push_str(&lines);
            }
            _ => fields.push(EmbedField {
                inline: false,
                name: day,
                value: lines,
            }),
        }
    }
    let footer = match (shown.is_empty(), hidden > 0) {
        (_, true) => Some(SCHEDULE_FOOTER_HIDDEN),
        (false, false) => Some(SCHEDULE_FOOTER),
        (true, false) => None,
    };
    if shown.is_empty() {
        description.push('\n');
        description.push_str(week.empty);
    }
    Embed {
        author: None,
        color: Some(INK_BLUE),
        description: Some(description),
        fields,
        footer: footer.map(|text| EmbedFooter {
            icon_url: None,
            proxy_icon_url: None,
            text: text.to_owned(),
        }),
        image: None,
        kind: "rich".to_owned(),
        provider: None,
        thumbnail: None,
        timestamp: None,
        title: Some(title(week.scope, week.next)),
        url: None,
        video: None,
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn the_range_names_the_month_once_when_it_can() {
        let zone = chrono_tz::Asia::Kuala_Lumpur;
        // Weeks starting Thursday 00:00 in the guild zone.
        let thu = |month, day| Utc.with_ymd_and_hms(2026, month, day, 16, 0, 0).unwrap();
        assert_eq!(range(thu(9, 2), zone), "Thu 03 → Wed 09 Sep");
        assert_eq!(range(thu(9, 23), zone), "Thu 24 → Wed 30 Sep");
        assert_eq!(range(thu(9, 30), zone), "Thu 01 → Wed 07 Oct");
        assert_eq!(range(thu(10, 28), zone), "Thu 29 Oct → Wed 04 Nov");
    }
}
