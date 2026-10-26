//! Outbox notice text, rendered by `cards::redesign::notice_text`. While
//! the public portal is open (read live per render) the `via_portal` mark
//! is a masked link into it (v5); closed, it is v4's.
//!
//! People are rendered through v4's `Audience` before the quiet-mode gate
//! (v4 named mentions in the text and emptied only the allow-list); the
//! allow-list is the intent's, already quiet-gated. Bosses and slots come
//! from the schedule at drain time; a notice whose run or timing is gone
//! renders nothing.

use std::collections::BTreeMap;

use chrono_tz::Tz;

use crate::cards::CardKit;
use crate::cards::format::Audience;
use crate::cards::redesign::{self, NoticeLook};
use crate::discord::mentions;
use crate::discord::transport::OutgoingMessage;
use kanade_domain::members::Directory;
use kanade_domain::notify::{NotificationIntent, PingKind, display_names, resolve_mentions};
use kanade_domain::schedule::{Notice, ScheduleSnapshot};

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

/// The post for an outbox `notice` planned as `intent`, or `None` when the
/// notice posts nothing now (see the module docs).
pub fn render_notice(
    notice: &Notice,
    intent: &NotificationIntent,
    schedule: &ScheduleSnapshot,
    members: &dyn Directory,
    zone: Tz,
    quiet: bool,
    kit: &CardKit,
) -> Option<OutgoingMessage> {
    let who = audience(notice, members);
    let look = NoticeLook {
        schedule,
        zone,
        catalog: kit.catalog.as_deref(),
        marks: &kit.marks,
        quiet,
        portal: kit.v2.portal_url(),
    };
    let content = redesign::notice_text(notice, &look, &who)?;
    Some(OutgoingMessage {
        content: Some(content),
        embeds: Vec::new(),
        allowed_mentions: mentions::for_intent(intent),
        reply_to: None,
        attachments: Vec::new(),
        components: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveTime, TimeZone, Utc, Weekday};

    use super::*;
    use kanade_domain::members::{Member, PingLevel, Roster};
    use kanade_domain::notify::{EffectKind, IntentContent};
    use kanade_domain::schedule::{FixedField, FixedRun, NoticeChange, Run, RunSource, RunStatus};

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
                owner_pinned: false,
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

    #[test]
    fn nothing_renders_for_a_derived_status_or_a_vanished_run() {
        let notice = |change| Notice {
            change,
            channel_id: None,
            listed: Vec::new(),
            via_portal: true,
        };
        let render = |notice: &Notice| {
            render_notice(
                notice,
                &intent(),
                &schedule(),
                &roster(),
                zone(),
                false,
                &CardKit::default(),
            )
        };
        let derived = NoticeChange::RunStatus {
            run_id: "r".into(),
            from: RunStatus::Planned,
            to: RunStatus::AtRisk,
        };
        assert_eq!(render(&notice(derived)), None);
        let gone = NoticeChange::RunStatus {
            run_id: "gone".into(),
            from: RunStatus::Planned,
            to: RunStatus::Cancelled,
        };
        assert_eq!(render(&notice(gone)), None);
        let timing_gone = NoticeChange::FixedChanged {
            fixed_id: "gone".into(),
            fields: vec![FixedField::Time],
            weekday: Weekday::Wed,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: Vec::new(),
        };
        assert_eq!(render(&notice(timing_gone)), None);
    }
}
