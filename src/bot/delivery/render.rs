//! Minimal plain-text cards. Full v4 card formatting (embeds, portraits,
//! quiet-mode lines) is a later slice; this is only enough to post.
//!
//! v5 attendance mode adds each run's tally (`4/4 (2 assumed)`) and, for a
//! confirmation resting on assumed answers, the label `expected`; v4-compat
//! renders exactly as before.

use chrono::{DateTime, Utc};

use crate::bot::mentions;
use crate::bot::transport::OutgoingMessage;
use crate::domain::attendance::{
    AttendanceMode, AttendancePolicy, Tally, snapshot_states, status_label,
};
use crate::domain::notify::{IntentContent, NotificationIntent};
use crate::domain::schedule::{NoticeChange, Run, ScheduleSnapshot};

fn stamp(at: DateTime<Utc>, style: char) -> String {
    format!("<t:{}:{style}>", at.timestamp())
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

fn run_line(run_id: &str, schedule: &ScheduleSnapshot, attendance: AttendancePolicy) -> String {
    match schedule.runs.iter().find(|run| run.id == run_id) {
        Some(run) => {
            let line = format!("{} {}", run.bosses.join(", "), stamp(run.datetime, 't'));
            match attendance.mode {
                AttendanceMode::V4Compat => line,
                AttendanceMode::V5 => {
                    format!("{line} · {}", tally_text(schedule, run, attendance))
                }
            }
        }
        None => "a run".to_owned(),
    }
}

/// Mention tags for the intent's allow-list. Quiet mode renders none, even
/// defensively: the quiet state is for admins, never announced publicly.
fn people(intent: &NotificationIntent, quiet: bool) -> String {
    if quiet {
        return String::new();
    }
    intent
        .mentions
        .iter()
        .map(|id| format!("<@{id}>"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The message for `intent`; its allow-list is exactly the intent's mentions.
/// `schedule` holds the runs it names (with their answers and timings).
pub fn render(
    intent: &NotificationIntent,
    schedule: &ScheduleSnapshot,
    attendance: AttendancePolicy,
    quiet: bool,
) -> OutgoingMessage {
    let line = |id: &String| format!("• {}", run_line(id, schedule, attendance));
    let body = match &intent.content {
        IntentContent::DayOf { run_ids } => {
            let lines: Vec<String> = run_ids.iter().map(line).collect();
            format!("Today's runs:\n{}", lines.join("\n"))
        }
        IntentContent::Countdown { run_id, minutes } => {
            format!(
                "{} starts in {minutes} min.",
                run_line(run_id, schedule, attendance)
            )
        }
        IntentContent::Digest {
            week_start,
            inclusion,
        } => {
            let head = format!(
                "Boss week from {}: {} runs, {} cleared, {} unsettled ({} at risk).",
                stamp(*week_start, 'D'),
                inclusion.live,
                inclusion.cleared,
                inclusion.unsettled,
                inclusion.at_risk
            );
            match attendance.mode {
                AttendanceMode::V4Compat => head,
                // v5: each live run of the week with its tally.
                AttendanceMode::V5 => {
                    let lines: Vec<String> = schedule
                        .runs
                        .iter()
                        .filter(|run| run.week_start == *week_start && run.status.is_live())
                        .map(|run| line(&run.id))
                        .collect();
                    if lines.is_empty() {
                        head
                    } else {
                        format!("{head}\n{}", lines.join("\n"))
                    }
                }
            }
        }
        IntentContent::Notice(notice) => match &notice.change {
            NoticeChange::Merged { title, run_ids, .. } => {
                let lines: Vec<String> = run_ids.iter().map(line).collect();
                let head = format!("Schedule update merged: {title}");
                if lines.is_empty() {
                    head
                } else {
                    format!("{head}\n{}", lines.join("\n"))
                }
            }
            NoticeChange::RequestDecided {
                decision, reason, ..
            } => match reason {
                Some(reason) => format!("Your request was {}: {reason}", decision.as_str()),
                None => format!("Your request was {}.", decision.as_str()),
            },
            _ => "The schedule changed.".to_owned(),
        },
        // Rendered by their senders (`bot::cards`), never by the tick.
        IntentContent::ProposalCard { .. } | IntentContent::Plain => String::new(),
    };
    let people = people(intent, quiet);
    let content = if people.is_empty() {
        body
    } else {
        format!("{body}\n{people}")
    };
    OutgoingMessage {
        content: Some(content),
        embeds: Vec::new(),
        allowed_mentions: mentions::for_intent(intent),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::attendance::StatusPin;
    use crate::domain::schedule::{RunSource, RunStatus};

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
        let run = &pinned.runs[0];
        assert_eq!(
            tally_text(&pinned, run, AttendancePolicy::V5),
            "0/2, confirmed (set by admin)"
        );
        let plain = schedule(None);
        assert_eq!(
            tally_text(&plain, &plain.runs[0], AttendancePolicy::V5),
            "0/2"
        );
        assert_eq!(
            run_line("r", &pinned, AttendancePolicy::V4_COMPAT),
            run_line("r", &plain, AttendancePolicy::V4_COMPAT),
            "v4-compat ignores the pin"
        );
    }
}
