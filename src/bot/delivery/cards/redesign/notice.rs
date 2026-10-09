//! The redesigned change and decline notices: plain text in one grammar
//! (emoji, bosses, what happened, who) with everything else (old slot,
//! source, quiet mode, react hint) in one `-#` subtext line. New times are
//! full Discord timestamps; weekly timings read as their result. Who is
//! tagged is the classic audience's choice and the allow-list stays the
//! caller's; in quiet mode nobody is tagged and the names move to the
//! subtext.

use chrono::{DateTime, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Tz;

use crate::bot::cards::format::{Audience, format_participants};
use crate::domain::catalog::BossTable;
use crate::domain::ids::short_id;
use crate::domain::schedule::{
    EMOJI_NO, EMOJI_YES, FixedField, Notice, NoticeChange, RequestDecision, Run, RunStatus,
    ScheduleSnapshot,
};

use super::super::common::{local_day, local_time};
use super::vocab::{DifficultyMarks, boss_labels, full_time, relative_time, subtext};

const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const QUIET: &str = "🔕 quiet mode, nobody was pinged";

/// What a redesigned change notice reads besides the notice itself.
#[derive(Clone, Copy)]
pub struct NoticeLook<'a> {
    pub schedule: &'a ScheduleSnapshot,
    pub zone: Tz,
    pub catalog: Option<&'a BossTable>,
    pub marks: &'a DifficultyMarks,
    pub quiet: bool,
}

/// One notice taken apart: the line, whom it names after the dash, the
/// run list under it and the subtext facts before source/hint/quiet.
struct Parts<'n> {
    head: String,
    named: &'n [String],
    lines: String,
    facts: Vec<String>,
    react: bool,
}

impl<'n> Parts<'n> {
    fn new(head: String, named: &'n [String]) -> Self {
        Self {
            head,
            named,
            lines: String::new(),
            facts: Vec::new(),
            react: false,
        }
    }

    fn fact(mut self, fact: String) -> Self {
        self.facts.push(fact);
        self
    }

    fn react(mut self) -> Self {
        self.react = true;
        self
    }
}

/// Names only, never a tag: quiet mode. A nameless member stays a tag,
/// which cannot ping with an empty allow-list.
fn names(users: &[String], who: &Audience, separator: &str) -> String {
    users
        .iter()
        .map(|user| match who.names.get(user) {
            Some(name) if !name.is_empty() => name.clone(),
            _ => format!("<@{user}>"),
        })
        .collect::<Vec<_>>()
        .join(separator)
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn weekly(weekday: Weekday, time: NaiveTime) -> String {
    format!(
        "**{} {:02}:{:02}**",
        WEEKDAYS[weekday.num_days_from_monday() as usize],
        time.hour(),
        time.minute()
    )
}

/// The redesigned text for `notice`, or `None` when it posts nothing (a
/// derived status, a vanished run or weekly timing). `who` is the classic
/// audience: names, and the tags the mention policy chose.
pub fn notice_text(notice: &Notice, look: &NoticeLook<'_>, who: &Audience) -> Option<String> {
    let zone = look.zone;
    let label = |bosses: &[String]| boss_labels(bosses, look.catalog, look.marks);
    let run = |id: &str| look.schedule.runs.iter().find(|run| run.id == id);
    let slot = |at: DateTime<Utc>| format!("{} {}", local_day(at, zone), local_time(at, zone));
    let people = |users: &[String]| {
        if look.quiet {
            names(users, who, " ")
        } else {
            format_participants(users, Some(who))
        }
    };
    let run_lines = |ids: &[String]| -> String {
        ids.iter()
            .filter_map(|id| run(id))
            .map(|run| format!("\n• {} · {}", label(&run.bosses), full_time(run.datetime)))
            .collect()
    };
    let listed = notice.listed.as_slice();
    let parts = match &notice.change {
        NoticeChange::RunStatus { run_id, to, .. } => {
            let run = run(run_id)?;
            let bosses = label(&run.bosses);
            match to {
                RunStatus::Cancelled => {
                    Parts::new(format!("🚫 {bosses} is cancelled"), listed).fact(slot(run.datetime))
                }
                RunStatus::Otot => Parts::new(format!("🕒 {bosses} is own time this week"), listed)
                    .fact(local_day(run.datetime, zone))
                    .fact("still in the morning ping, no countdowns".to_owned()),
                RunStatus::Done => {
                    Parts::new(format!("🏁 {bosses} cleared"), listed).fact(slot(run.datetime))
                }
                RunStatus::Planned => Parts::new(
                    format!("🔁 {bosses} is back on for **{}**", full_time(run.datetime)),
                    listed,
                )
                .react(),
                RunStatus::Confirmed => Parts::new(
                    format!(
                        "✅ {bosses} is confirmed for **{}**",
                        full_time(run.datetime)
                    ),
                    listed,
                ),
                RunStatus::AtRisk => return None,
            }
        }
        NoticeChange::RunMoved { run_id, from, to } => {
            let run = run(run_id)?;
            Parts::new(
                format!("🔁 {} moved to **{}**", label(&run.bosses), full_time(*to)),
                listed,
            )
            .fact(format!("was {}", slot(*from)))
            .react()
        }
        NoticeChange::RunSwapped {
            run_id,
            leaving,
            joining,
            ..
        } => {
            let run = run(run_id)?;
            let change = match (joining.is_empty(), leaving.is_empty()) {
                (false, false) => format!("{} in for {}", people(joining), people(leaving)),
                (false, true) => format!("{} in", people(joining)),
                (true, false) => format!("{} out", people(leaving)),
                (true, true) => "line-up changed".to_owned(),
            };
            Parts::new(
                format!("🔁 {} this week: {change}", label(&run.bosses)),
                &[],
            )
            .fact(slot(run.datetime))
            .fact("weekly party unchanged".to_owned())
        }
        NoticeChange::RunReset { run_id, from, to } => {
            let run = run(run_id)?;
            Parts::new(
                format!(
                    "🔁 {} is back on its weekly timing: **{}**",
                    label(&run.bosses),
                    full_time(*to)
                ),
                listed,
            )
            .fact(format!("was {}", slot(*from)))
            .react()
        }
        NoticeChange::FixedChanged {
            fixed_id,
            fields,
            weekday,
            time,
            ..
        } if fields.as_slice() == [FixedField::OwnerId] => {
            let fixed = look
                .schedule
                .fixed_runs
                .iter()
                .find(|row| &row.id == fixed_id)?;
            Parts::new(
                format!(
                    "👑 {} every {} has a new owner",
                    label(&fixed.bosses),
                    weekly(*weekday, *time)
                ),
                &notice.listed,
            )
        }
        NoticeChange::FixedChanged {
            fixed_id,
            weekday,
            time,
            participants,
            ..
        } => {
            let fixed = look
                .schedule
                .fixed_runs
                .iter()
                .find(|row| &row.id == fixed_id)?;
            Parts::new(
                format!(
                    "📌 {} is now every {}",
                    label(&fixed.bosses),
                    weekly(*weekday, *time)
                ),
                participants,
            )
        }
        NoticeChange::FixedAdded {
            bosses,
            weekday,
            time,
            participants,
            ..
        } => Parts::new(
            format!(
                "📌 {} is now every {}",
                label(bosses),
                weekly(*weekday, *time)
            ),
            participants,
        )
        .fact("new weekly timing".to_owned()),
        NoticeChange::FixedRemoved {
            bosses,
            weekday,
            time,
            participants,
            cancelled_runs,
            ..
        } => {
            let parts = Parts::new(
                format!(
                    "🗑️ {} no longer runs every {}",
                    label(bosses),
                    weekly(*weekday, *time)
                ),
                participants,
            );
            if *cancelled_runs > 0 {
                parts.fact(format!(
                    "{} cancelled",
                    plural(*cancelled_runs, "run", "runs")
                ))
            } else {
                parts
            }
        }
        NoticeChange::Rollback {
            reverted,
            run_ids,
            checkpoint,
            ..
        } => {
            let head = match checkpoint {
                Some(name) => format!("↩️ Schedule restored to checkpoint **{name}**"),
                None => "↩️ Schedule rolled back".to_owned(),
            };
            Parts {
                lines: run_lines(run_ids),
                ..Parts::new(head, &[])
            }
            .fact(format!(
                "{} undone",
                plural(reverted.len(), "change", "changes")
            ))
        }
        NoticeChange::Merged { title, run_ids, .. } => Parts {
            lines: run_lines(run_ids),
            ..Parts::new(format!("📝 Schedule updated: {title}"), &[])
        },
        NoticeChange::RequestDecided {
            decision, reason, ..
        } => {
            let head = match (decision, reason) {
                (RequestDecision::Rejected, Some(reason)) => {
                    format!("📨 Request rejected: {reason}")
                }
                (decision, _) => format!("📨 Request {}", decision.as_str()),
            };
            Parts::new(head, listed)
        }
    };
    Some(assemble(parts, notice.via_portal, look.quiet, who))
}

fn assemble(parts: Parts<'_>, via_portal: bool, quiet: bool, who: &Audience) -> String {
    let Parts {
        mut head,
        named,
        lines,
        mut facts,
        react,
    } = parts;
    if !quiet && !named.is_empty() {
        head.push_str(" — ");
        head.push_str(&format_participants(named, Some(who)));
    }
    head.push_str(&lines);
    if via_portal {
        facts.push("via portal".to_owned());
    }
    if react {
        facts.push(format!("react {EMOJI_YES}/{EMOJI_NO} here"));
    }
    if quiet {
        facts.push(QUIET.to_owned());
        if !named.is_empty() {
            facts.push(names(named, who, ", "));
        }
    }
    if !facts.is_empty() {
        head.push('\n');
        head.push_str(&subtext(&facts.join(" · ")));
    }
    head
}

/// The redesigned decline notice: the decliner leads, the run's time is
/// relative, and the people told (`recipients`, already rendered with
/// their tags) move to the subtext beside `/amend` and `/swap`.
pub fn decline_text(
    decliner: &str,
    run: &Run,
    recipients: &str,
    catalog: Option<&BossTable>,
    marks: &DifficultyMarks,
) -> String {
    let mut notes = Vec::new();
    if !recipients.is_empty() {
        notes.push(format!("for {recipients}"));
    }
    notes.push(format!("`/amend run_id:{}`", short_id(&run.id)));
    notes.push("`/swap`".to_owned());
    format!(
        "❌ **{decliner}** can't make {} {}. Eh? Reschedule, or find a stand-in?\n{}",
        boss_labels(&run.bosses, catalog, marks),
        relative_time(run.datetime),
        subtext(&notes.join(" · "))
    )
}
