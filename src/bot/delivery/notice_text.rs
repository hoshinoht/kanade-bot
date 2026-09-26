//! Outbox notice text. Kinds v4 announced render v4's
//! `bot.agent.formatting` notices byte for byte (`amend_notice`,
//! `cancel_notice`, `otot_notice`, `done_notice`, `restore_notice`, the
//! `confirmed` line of `service.status_notice`, `swap_notice`,
//! `fixed_notice`), with `via_portal` and `quiet_line` appended as v4's
//! `_announce` and `send_operation_payload` did. Rollback, reset, merge and
//! request-decision notices are v5-only and keep short texts of their own.
//!
//! People are rendered through v4's `Audience` before the quiet-mode gate
//! (v4 named mentions in the text and emptied only the allow-list); the
//! allow-list is the intent's, already quiet-gated. Bosses and slots come
//! from the schedule at drain time; a notice whose run or timing is gone
//! renders nothing.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
use chrono_tz::Tz;

use crate::bot::cards::format::{Audience, format_participants};
use crate::bot::mentions;
use crate::bot::transport::OutgoingMessage;
use crate::domain::members::Directory;
use crate::domain::notify::{NotificationIntent, PingKind, display_names, resolve_mentions};
use crate::domain::schedule::{
    EMOJI_NO, EMOJI_YES, Notice, NoticeChange, RequestDecision, RunStatus, ScheduleSnapshot,
};

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTH_NAMES: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
/// v4 `VIA_PORTAL`.
const VIA_PORTAL: &str = "_(via portal)_";
/// v4 `QUIET_NOTE`.
const QUIET_NOTE: &str = "🔕 quiet mode - nobody was notified";

fn react_hint() -> String {
    format!("React {EMOJI_YES} if you're on, {EMOJI_NO} if not.")
}

/// v4 `format_bosses`: stored tokens, not labels.
fn format_bosses(bosses: &[String]) -> String {
    if bosses.is_empty() {
        "(no bosses)".to_owned()
    } else {
        bosses.join(" + ")
    }
}

fn local_day(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!(
        "{} {:02} {}",
        WEEKDAY_NAMES[local.weekday().num_days_from_monday() as usize],
        local.day(),
        MONTH_NAMES[local.month0() as usize]
    )
}

fn local_time(at: DateTime<Utc>, zone: Tz) -> String {
    let local = at.with_timezone(&zone);
    format!("{:02}:{:02}", local.hour(), local.minute())
}

fn slot(at: DateTime<Utc>, zone: Tz) -> String {
    format!("{} {}", local_day(at, zone), local_time(at, zone))
}

fn weekday_name(weekday: Weekday) -> &'static str {
    WEEKDAY_NAMES[weekday.num_days_from_monday() as usize]
}

/// v4 `pings.audience(repo, listed, kind)`.
fn audience(notice: &Notice, members: &dyn Directory) -> Audience {
    let kind = PingKind::parse(notice.ping_kind());
    Audience {
        names: display_names(members, &notice.listed)
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
        mentioned: resolve_mentions(members, &notice.listed, &kind),
    }
}

/// The body before the portal and quiet suffixes; `None` posts nothing.
fn body(notice: &Notice, schedule: &ScheduleSnapshot, who: &Audience, zone: Tz) -> Option<String> {
    let people = |ids: &[String]| format_participants(ids, Some(who));
    let run = |id: &str| schedule.runs.iter().find(|run| run.id == id);
    let run_lines = |ids: &[String]| -> String {
        ids.iter()
            .filter_map(|id| run(id))
            .map(|run| {
                format!(
                    "\n• **{}** {}",
                    format_bosses(&run.bosses),
                    slot(run.datetime, zone)
                )
            })
            .collect()
    };
    Some(match &notice.change {
        NoticeChange::RunStatus { run_id, to, .. } => {
            let run = run(run_id)?;
            let bosses = format_bosses(&run.bosses);
            let who = people(&notice.listed);
            match to {
                RunStatus::Cancelled => format!(
                    "🚫 **{bosses}** ({}) is cancelled — {who}",
                    local_day(run.datetime, zone)
                ),
                RunStatus::Otot => format!(
                    "🕒 **{bosses}** is own-time this week ({}) — it stays in the morning \
                     ping, but there are no countdowns. {who}",
                    local_day(run.datetime, zone)
                ),
                RunStatus::Done => format!("🏁 **{bosses}** cleared — {who}"),
                RunStatus::Planned => format!(
                    "🔁 **{bosses}** is back on the schedule ({}) — {who}\n{}",
                    slot(run.datetime, zone),
                    react_hint()
                ),
                RunStatus::Confirmed => format!(
                    "✅ **{bosses}** is confirmed for {} — {who}",
                    slot(run.datetime, zone)
                ),
                // v4 `status_notice` stays quiet for anything else.
                RunStatus::AtRisk => return None,
            }
        }
        NoticeChange::RunMoved { run_id, from, to } => {
            let run = run(run_id)?;
            format!(
                "🔁 **{}** moved: ~~{}~~ → **{}** — {}\n{}",
                format_bosses(&run.bosses),
                slot(*from, zone),
                slot(*to, zone),
                people(&notice.listed),
                react_hint()
            )
        }
        NoticeChange::RunSwapped {
            run_id,
            leaving,
            joining,
            ..
        } => {
            let run = run(run_id)?;
            let mut parts = Vec::new();
            if !leaving.is_empty() {
                parts.push(format!("{} out", people(leaving)));
            }
            if !joining.is_empty() {
                parts.push(format!("{} in", people(joining)));
            }
            format!(
                "🔁 **{}** ({}): {} this week — the weekly timing is unchanged.",
                format_bosses(&run.bosses),
                slot(run.datetime, zone),
                parts.join(" · ")
            )
        }
        NoticeChange::FixedChanged {
            fixed_id,
            weekday,
            time,
            participants,
            ..
        } => {
            let fixed = schedule.fixed_runs.iter().find(|row| &row.id == fixed_id)?;
            format!(
                "📌 Weekly timing changed: **{}** · {} {:02}:{:02} · {}",
                format_bosses(&fixed.bosses),
                weekday_name(*weekday),
                time.hour(),
                time.minute(),
                people(participants)
            )
        }
        // v5 only from here on.
        NoticeChange::RunReset { run_id, from, to } => {
            let run = run(run_id)?;
            format!(
                "🔁 **{}** is back on its weekly timing: ~~{}~~ → **{}** — {}\n{}",
                format_bosses(&run.bosses),
                slot(*from, zone),
                slot(*to, zone),
                people(&notice.listed),
                react_hint()
            )
        }
        NoticeChange::Rollback {
            reverted,
            run_ids,
            checkpoint,
            ..
        } => {
            let head = match checkpoint {
                Some(name) => format!("↩️ Schedule restored to checkpoint **{name}**"),
                None => format!(
                    "↩️ Schedule rolled back ({} change(s) undone)",
                    reverted.len()
                ),
            };
            format!("{head}{}", run_lines(run_ids))
        }
        NoticeChange::Merged { title, run_ids, .. } => {
            format!("📝 Schedule updated: {title}{}", run_lines(run_ids))
        }
        NoticeChange::RequestDecided {
            decision, reason, ..
        } => {
            let who = people(&notice.listed);
            match (decision, reason) {
                (RequestDecision::Rejected, Some(reason)) => {
                    format!("📨 {who} your request was rejected: {reason}")
                }
                (decision, _) => format!("📨 {who} your request was {}.", decision.as_str()),
            }
        }
    })
}

/// The post for an outbox `notice` planned as `intent`, or `None` when the
/// notice posts nothing now (see the module docs).
pub fn render_notice(
    notice: &Notice,
    intent: &NotificationIntent,
    schedule: &ScheduleSnapshot,
    members: &dyn Directory,
    zone: Tz,
    quiet: bool,
) -> Option<OutgoingMessage> {
    let who = audience(notice, members);
    let mut content = body(notice, schedule, &who, zone)?;
    if notice.via_portal {
        content = format!("{content}\n{VIA_PORTAL}");
    }
    if quiet {
        content = format!("{content}\n_{QUIET_NOTE}_");
    }
    Some(OutgoingMessage {
        content: Some(content),
        embeds: Vec::new(),
        allowed_mentions: mentions::for_intent(intent),
        reply_to: None,
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeZone};

    use super::*;
    use crate::domain::members::{Member, PingLevel, Roster};
    use crate::domain::notify::{EffectKind, IntentContent};
    use crate::domain::schedule::{FixedField, FixedRun, Run, RunSource};

    fn zone() -> Tz {
        chrono_tz::Asia::Kuala_Lumpur
    }

    /// Alvin is named, kanon wants pings, 1003 is a stranger (default level).
    fn roster() -> Roster {
        let mut roster = Roster::new();
        for (id, name, level) in [
            ("1001", "Alvin", PingLevel::Off),
            ("1002", "kanon", PingLevel::All),
        ] {
            roster.upsert(Member {
                user_id: id.into(),
                display_name: Some(name.into()),
                has_role: true,
                ping_level: level,
                ..Member::default()
            });
        }
        roster
    }

    fn schedule() -> ScheduleSnapshot {
        let at = Utc.with_ymd_and_hms(2026, 9, 10, 13, 30, 0).unwrap();
        ScheduleSnapshot {
            runs: vec![Run {
                id: "r".into(),
                fixed_run_id: None,
                channel_id: Some("222".into()),
                week_start: at,
                datetime: at,
                bosses: vec!["HMaleficStar".into(), "HFA".into()],
                participants: vec!["1001".into(), "1002".into(), "1003".into()],
                status: RunStatus::Planned,
                source: RunSource::Amend,
                attendance: Vec::new(),
                status_pin: None,
            }],
            fixed_runs: vec![FixedRun {
                id: "f".into(),
                owner_id: "1001".into(),
                channel_id: Some("222".into()),
                bosses: vec!["HFA".into()],
                weekday: Weekday::Wed,
                time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
                participants: vec!["1001".into(), "1002".into()],
                note: None,
                attendance_default: Default::default(),
                standing: Vec::new(),
            }],
            ..ScheduleSnapshot::default()
        }
    }

    fn intent() -> NotificationIntent {
        NotificationIntent {
            effect: EffectKind::Notice("n".into()),
            effect_context: Vec::new(),
            channel_id: "222".into(),
            targets: Vec::new(),
            mentions: vec!["1002".into()],
            content: IntentContent::Plain,
            warnings: Vec::new(),
        }
    }

    fn text(change: NoticeChange, listed: &[&str], via_portal: bool, quiet: bool) -> String {
        let notice = Notice {
            change,
            channel_id: Some("222".into()),
            listed: listed.iter().map(|id| (*id).to_owned()).collect(),
            via_portal,
        };
        render_notice(&notice, &intent(), &schedule(), &roster(), zone(), quiet)
            .expect("renders")
            .content
            .expect("text")
    }

    fn status(to: RunStatus) -> NoticeChange {
        NoticeChange::RunStatus {
            run_id: "r".into(),
            from: RunStatus::Planned,
            to,
        }
    }

    const PARTY: [&str; 3] = ["1001", "1002", "1003"];

    /// Expected strings printed by v4 `bot.agent.formatting` itself (the
    /// rollback tree, `uv run`), for the same run, audience and zone.
    #[test]
    fn v4_announced_kinds_match_v4_bytes() {
        let old = Utc.with_ymd_and_hms(2026, 9, 9, 13, 0, 0).unwrap();
        let new = Utc.with_ymd_and_hms(2026, 9, 10, 13, 30, 0).unwrap();
        let cases = [
            (
                NoticeChange::RunMoved {
                    run_id: "r".into(),
                    from: old,
                    to: new,
                },
                "🔁 **HMaleficStar + HFA** moved: ~~Wed 09 Sep 21:00~~ → **Thu 10 Sep 21:30** — \
                 Alvin <@1002> <@1003>\nReact ✅ if you're on, ❌ if not.",
            ),
            (
                status(RunStatus::Cancelled),
                "🚫 **HMaleficStar + HFA** (Thu 10 Sep) is cancelled — Alvin <@1002> <@1003>",
            ),
            (
                status(RunStatus::Otot),
                "🕒 **HMaleficStar + HFA** is own-time this week (Thu 10 Sep) — it stays in \
                 the morning ping, but there are no countdowns. Alvin <@1002> <@1003>",
            ),
            (
                status(RunStatus::Done),
                "🏁 **HMaleficStar + HFA** cleared — Alvin <@1002> <@1003>",
            ),
            (
                status(RunStatus::Planned),
                "🔁 **HMaleficStar + HFA** is back on the schedule (Thu 10 Sep 21:30) — Alvin \
                 <@1002> <@1003>\nReact ✅ if you're on, ❌ if not.",
            ),
            (
                status(RunStatus::Confirmed),
                "✅ **HMaleficStar + HFA** is confirmed for Thu 10 Sep 21:30 — Alvin <@1002> \
                 <@1003>",
            ),
        ];
        for (change, expected) in cases {
            assert_eq!(text(change, &PARTY, false, false), expected);
        }
        let swap = NoticeChange::RunSwapped {
            run_id: "r".into(),
            participants: vec!["1001".into(), "1002".into()],
            leaving: vec!["1003".into()],
            joining: vec!["1002".into()],
        };
        assert_eq!(
            text(swap, &PARTY, false, false),
            "🔁 **HMaleficStar + HFA** (Thu 10 Sep 21:30): <@1003> out · <@1002> in this week \
             — the weekly timing is unchanged."
        );
        let fixed = NoticeChange::FixedChanged {
            fixed_id: "f".into(),
            fields: vec![FixedField::Time],
            weekday: Weekday::Wed,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
        };
        assert_eq!(
            text(fixed, &["1001", "1002"], false, false),
            "📌 Weekly timing changed: **HFA** · Wed 21:30 · Alvin <@1002>"
        );
    }

    #[test]
    fn portal_and_quiet_suffixes_follow_v4() {
        assert_eq!(
            text(status(RunStatus::Done), &PARTY, true, true),
            "🏁 **HMaleficStar + HFA** cleared — Alvin <@1002> <@1003>\n_(via portal)_\n\
             _🔕 quiet mode - nobody was notified_"
        );
    }

    #[test]
    fn nothing_renders_for_a_derived_status_or_a_vanished_run() {
        let notice = |change| Notice {
            change,
            channel_id: None,
            listed: Vec::new(),
            via_portal: true,
        };
        let render = |notice: &Notice| {
            render_notice(notice, &intent(), &schedule(), &roster(), zone(), false)
        };
        assert_eq!(render(&notice(status(RunStatus::AtRisk))), None);
        let gone = NoticeChange::RunStatus {
            run_id: "gone".into(),
            from: RunStatus::Planned,
            to: RunStatus::Cancelled,
        };
        assert_eq!(render(&notice(gone)), None);
    }
}
