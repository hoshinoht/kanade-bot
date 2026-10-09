//! Change notices in both message styles: every kind in the redesigned
//! grammar (emoji, bosses, what happened, who; the rest in one subtext
//! line), the classic texts unchanged, and the same allow-list either way,
//! rendered directly and drained by the tick.

use std::sync::Arc;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::bot::delivery::cards::CardKit;
use kanade::bot::delivery::cards::redesign::DifficultyMarks;
use kanade::bot::delivery::render_notice;
use kanade::bot::transport::OutgoingMessage;
use kanade::domain::history::Origin;
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::notify::{EffectKind, IntentContent, NotificationIntent};
use kanade::domain::schedule::{
    FixedField, FixedRun, NewRun, Notice, NoticeChange, RequestDecision, Run, RunSource, RunStatus,
    ScheduleSnapshot, StatusChange,
};
use kanade::domain::settings::MessageStyle;
use kanade::infrastructure::store::MemoryScheduleStore;

use super::ZONE;
use crate::cards::{created, kit, world};
use crate::scenarios::{HOME, config, delivery, now, week};
use crate::support;

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
}

/// The test catalog in `style`, read live like serve reads the setting.
pub(crate) fn styled(style: MessageStyle) -> CardKit {
    CardKit {
        style: Some(Arc::new(move || style)),
        ..kit(None)
    }
}

/// Alvin is named (no pings), kanon wants pings, 1003 is a nameless
/// stranger.
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

fn wed_time() -> NaiveTime {
    NaiveTime::from_hms_opt(21, 30, 0).unwrap()
}

fn ids(users: &[&str]) -> Vec<String> {
    users.iter().map(|user| (*user).to_owned()).collect()
}

const PARTY: [&str; 3] = ["1001", "1002", "1003"];
const WEEKLY: [&str; 2] = ["1001", "1002"];

/// Run `r` on Thu 10 Sep 21:30 (`<t:1789047000>`) and its weekly timing
/// `f`, Wednesdays 21:30.
fn schedule() -> ScheduleSnapshot {
    ScheduleSnapshot {
        runs: vec![Run {
            id: "r".into(),
            fixed_run_id: None,
            channel_id: Some("222".into()),
            week_start: utc(8, 16, 0),
            datetime: utc(10, 13, 30),
            bosses: ids(&["HMaleficStar"]),
            participants: ids(&PARTY),
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
            bosses: ids(&["HMaleficStar"]),
            weekday: Weekday::Wed,
            time: wed_time(),
            participants: ids(&WEEKLY),
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
        mentions: ids(&["1002"]),
        content: IntentContent::Plain,
        warnings: Vec::new(),
    }
}

struct Case {
    name: &'static str,
    change: NoticeChange,
    listed: &'static [&'static str],
    via_portal: bool,
    quiet: bool,
    classic: &'static str,
    redesigned: &'static str,
}

fn status(to: RunStatus) -> NoticeChange {
    NoticeChange::RunStatus {
        run_id: "r".into(),
        from: RunStatus::Planned,
        to,
    }
}

fn fixed_changed() -> NoticeChange {
    NoticeChange::FixedChanged {
        fixed_id: "f".into(),
        fields: vec![FixedField::Time],
        weekday: Weekday::Wed,
        time: wed_time(),
        participants: ids(&WEEKLY),
    }
}

fn moved() -> NoticeChange {
    NoticeChange::RunMoved {
        run_id: "r".into(),
        from: utc(9, 13, 0),
        to: utc(10, 13, 30),
    }
}

fn request(decision: RequestDecision, reason: Option<&str>) -> NoticeChange {
    NoticeChange::RequestDecided {
        request: "q".into(),
        decision,
        reason: reason.map(str::to_owned),
    }
}

/// Every notice kind; the classic column is today's text, byte for byte.
fn cases() -> Vec<Case> {
    let case = |name, change, listed, classic, redesigned| Case {
        name,
        change,
        listed,
        via_portal: false,
        quiet: false,
        classic,
        redesigned,
    };
    vec![
        case(
            "cancel",
            status(RunStatus::Cancelled),
            &PARTY,
            "🚫 **HMaleficStar** (Thu 10 Sep) is cancelled — Alvin <@1002> <@1003>",
            "🚫 Hard Radiant Malefic Star is cancelled — Alvin <@1002> <@1003>\n\
             -# Thu 10 Sep 21:30",
        ),
        case(
            "own time",
            status(RunStatus::Otot),
            &PARTY,
            "🕒 **HMaleficStar** is own-time this week (Thu 10 Sep) — it stays in the morning \
             ping, but there are no countdowns. Alvin <@1002> <@1003>",
            "🕒 Hard Radiant Malefic Star is own time this week — Alvin <@1002> <@1003>\n\
             -# Thu 10 Sep · still in the morning ping, no countdowns",
        ),
        case(
            "cleared",
            status(RunStatus::Done),
            &PARTY,
            "🏁 **HMaleficStar** cleared — Alvin <@1002> <@1003>",
            "🏁 Hard Radiant Malefic Star cleared — Alvin <@1002> <@1003>\n-# Thu 10 Sep 21:30",
        ),
        case(
            "back on",
            status(RunStatus::Planned),
            &PARTY,
            "🔁 **HMaleficStar** is back on the schedule (Thu 10 Sep 21:30) — Alvin <@1002> \
             <@1003>\nReact ✅ if you're on, ❌ if not.",
            "🔁 Hard Radiant Malefic Star is back on for **<t:1789047000:F>** — Alvin <@1002> \
             <@1003>\n-# react ✅/❌ here",
        ),
        case(
            "confirmed",
            status(RunStatus::Confirmed),
            &PARTY,
            "✅ **HMaleficStar** is confirmed for Thu 10 Sep 21:30 — Alvin <@1002> <@1003>",
            "✅ Hard Radiant Malefic Star is confirmed for **<t:1789047000:F>** — Alvin <@1002> \
             <@1003>",
        ),
        Case {
            via_portal: true,
            ..case(
                "moved",
                moved(),
                &PARTY,
                "🔁 **HMaleficStar** moved: ~~Wed 09 Sep 21:00~~ → **Thu 10 Sep 21:30** — Alvin \
                 <@1002> <@1003>\nReact ✅ if you're on, ❌ if not.\n_(via portal)_",
                "🔁 Hard Radiant Malefic Star moved to **<t:1789047000:F>** — Alvin <@1002> \
                 <@1003>\n-# was Wed 09 Sep 21:00 · via portal · react ✅/❌ here",
            )
        },
        Case {
            via_portal: true,
            quiet: true,
            ..case(
                "moved quietly",
                moved(),
                &PARTY,
                "🔁 **HMaleficStar** moved: ~~Wed 09 Sep 21:00~~ → **Thu 10 Sep 21:30** — Alvin \
                 <@1002> <@1003>\nReact ✅ if you're on, ❌ if not.\n_(via portal)_\n\
                 _🔕 quiet mode - nobody was notified_",
                "🔁 Hard Radiant Malefic Star moved to **<t:1789047000:F>**\n\
                 -# was Wed 09 Sep 21:00 · via portal · react ✅/❌ here · 🔕 quiet mode, \
                 nobody was pinged · Alvin, kanon, <@1003>",
            )
        },
        case(
            "stand-in",
            NoticeChange::RunSwapped {
                run_id: "r".into(),
                participants: ids(&["1002", "1003"]),
                leaving: ids(&["1001"]),
                joining: ids(&["1002"]),
            },
            &PARTY,
            "🔁 **HMaleficStar** (Thu 10 Sep 21:30): Alvin out · <@1002> in this week — the \
             weekly timing is unchanged.",
            "🔁 Hard Radiant Malefic Star this week: <@1002> in for Alvin\n\
             -# Thu 10 Sep 21:30 · weekly party unchanged",
        ),
        case(
            "weekly timing changed",
            fixed_changed(),
            &WEEKLY,
            "📌 Weekly timing changed: **HMaleficStar** · Wed 21:30 · Alvin <@1002>",
            "📌 Hard Radiant Malefic Star is now every **Wednesday 21:30** — Alvin <@1002>",
        ),
        Case {
            quiet: true,
            ..case(
                "weekly timing changed quietly",
                fixed_changed(),
                &WEEKLY,
                "📌 Weekly timing changed: **HMaleficStar** · Wed 21:30 · Alvin <@1002>\n\
                 _🔕 quiet mode - nobody was notified_",
                "📌 Hard Radiant Malefic Star is now every **Wednesday 21:30**\n\
                 -# 🔕 quiet mode, nobody was pinged · Alvin, kanon",
            )
        },
        case(
            "weekly timing added",
            NoticeChange::FixedAdded {
                fixed_id: "f".into(),
                bosses: ids(&["HMaleficStar"]),
                weekday: Weekday::Wed,
                time: wed_time(),
                participants: ids(&WEEKLY),
            },
            &WEEKLY,
            "📌 Weekly timing added: **HMaleficStar** · Wed 21:30 · Alvin <@1002>",
            "📌 Hard Radiant Malefic Star is now every **Wednesday 21:30** — Alvin <@1002>\n\
             -# new weekly timing",
        ),
        case(
            "weekly timing removed",
            NoticeChange::FixedRemoved {
                fixed_id: "f".into(),
                bosses: ids(&["HMaleficStar"]),
                weekday: Weekday::Wed,
                time: wed_time(),
                participants: ids(&WEEKLY),
                cancelled_runs: 2,
            },
            &WEEKLY,
            "🗑️ Weekly timing removed: **HMaleficStar** · Wed 21:30 · Alvin <@1002>",
            "🗑️ Hard Radiant Malefic Star no longer runs every **Wednesday 21:30** — Alvin \
             <@1002>\n-# 2 runs cancelled",
        ),
        case(
            "back on weekly timing",
            NoticeChange::RunReset {
                run_id: "r".into(),
                from: utc(9, 13, 0),
                to: utc(10, 13, 30),
            },
            &PARTY,
            "🔁 **HMaleficStar** is back on its weekly timing: ~~Wed 09 Sep 21:00~~ → **Thu 10 \
             Sep 21:30** — Alvin <@1002> <@1003>\nReact ✅ if you're on, ❌ if not.",
            "🔁 Hard Radiant Malefic Star is back on its weekly timing: **<t:1789047000:F>** — \
             Alvin <@1002> <@1003>\n-# was Wed 09 Sep 21:00 · react ✅/❌ here",
        ),
        case(
            "restore",
            NoticeChange::Rollback {
                reverted: vec![3, 2],
                cancelled_runs: Vec::new(),
                run_ids: ids(&["r"]),
                checkpoint: Some("before-raid".into()),
            },
            &[],
            "↩️ Schedule restored to checkpoint **before-raid**\n• **HMaleficStar** Thu 10 Sep \
             21:30",
            "↩️ Schedule restored to checkpoint **before-raid**\n\
             • Hard Radiant Malefic Star · <t:1789047000:F>\n-# 2 changes undone",
        ),
        Case {
            via_portal: true,
            ..case(
                "rollback",
                NoticeChange::Rollback {
                    reverted: vec![5],
                    cancelled_runs: Vec::new(),
                    run_ids: ids(&["r"]),
                    checkpoint: None,
                },
                &[],
                "↩️ Schedule rolled back (1 change(s) undone)\n• **HMaleficStar** Thu 10 Sep \
                 21:30\n_(via portal)_",
                "↩️ Schedule rolled back\n• Hard Radiant Malefic Star · <t:1789047000:F>\n\
                 -# 1 change undone · via portal",
            )
        },
        case(
            "schedule updated",
            NoticeChange::Merged {
                draft: "d".into(),
                version: 2,
                title: "retime the raid".into(),
                run_ids: ids(&["r"]),
                fixed_ids: Vec::new(),
            },
            &[],
            "📝 Schedule updated: retime the raid\n• **HMaleficStar** Thu 10 Sep 21:30",
            "📝 Schedule updated: retime the raid\n• Hard Radiant Malefic Star · \
             <t:1789047000:F>",
        ),
        case(
            "request approved",
            request(RequestDecision::Approved, None),
            &["1002"],
            "📨 <@1002> your request was approved.",
            "📨 Request approved — <@1002>",
        ),
        case(
            "request rejected",
            request(RequestDecision::Rejected, Some("the party is full")),
            &["1002"],
            "📨 <@1002> your request was rejected: the party is full",
            "📨 Request rejected: the party is full — <@1002>",
        ),
        case(
            "request expired",
            request(RequestDecision::Expired, None),
            &["1002"],
            "📨 <@1002> your request was expired.",
            "📨 Request expired — <@1002>",
        ),
    ]
}

fn render(case: &Case, kit: &CardKit) -> OutgoingMessage {
    let notice = Notice {
        change: case.change.clone(),
        channel_id: Some("222".into()),
        listed: ids(case.listed),
        via_portal: case.via_portal,
    };
    render_notice(
        &notice,
        &intent(),
        &schedule(),
        &roster(),
        ZONE,
        case.quiet,
        kit,
    )
    .unwrap_or_else(|| panic!("{} renders", case.name))
}

#[test]
fn every_notice_kind_reads_in_the_redesigned_grammar() {
    let kit = styled(MessageStyle::Redesigned);
    for case in cases() {
        let message = render(&case, &kit);
        assert_eq!(
            message.content.as_deref(),
            Some(case.redesigned),
            "{}",
            case.name
        );
        assert!(message.embeds.is_empty(), "{}: plain text", case.name);
    }
}

#[test]
fn classic_notices_are_unchanged_and_both_styles_ping_the_same_people() {
    let classic = styled(MessageStyle::Classic);
    let redesigned = styled(MessageStyle::Redesigned);
    for case in cases() {
        let old = render(&case, &classic);
        assert_eq!(old.content.as_deref(), Some(case.classic), "{}", case.name);
        assert_eq!(old, render(&case, &CardKit::default()), "{}", case.name);
        assert_eq!(
            old.allowed_mentions,
            render(&case, &redesigned).allowed_mentions,
            "{}",
            case.name
        );
    }
}

#[test]
fn difficulty_marks_label_the_bosses() {
    let kit = CardKit {
        marks: DifficultyMarks::new().with("h", "<:diff_h:77>"),
        ..styled(MessageStyle::Redesigned)
    };
    assert_eq!(
        render(&cases()[0], &kit).content.as_deref(),
        Some(
            "🚫 <:diff_h:77> Radiant Malefic Star is cancelled — Alvin <@1002> <@1003>\n\
             -# Thu 10 Sep 21:30"
        )
    );
}

/// A cancellation's outbox notice drained by the tick in each style: the
/// redesigned text, the same allow-list.
#[tokio::test]
async fn the_tick_drains_notices_in_the_live_style() {
    let mut posts = Vec::new();
    for style in [MessageStyle::Classic, MessageStyle::Redesigned] {
        let store = MemoryScheduleStore::new();
        let world = world();
        let mut generator = RandomIds;
        let mut service = support::service(&store, &mut generator, now());
        let run = service
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: None,
                channel_id: Some(HOME.into()),
                week_start: week(),
                datetime: now() + chrono::Duration::hours(3),
                bosses: ids(&["HKalos"]),
                participants: ids(&["1001", "1002"]),
                status: RunStatus::Planned,
                source: RunSource::Amend,
            })
            .await
            .expect("run");
        service
            .as_origin(Origin::for_tests())
            .set_status(
                &run,
                StatusChange {
                    status: RunStatus::Cancelled,
                    announce: true,
                    via_portal: true,
                },
                &config().policy.reminders,
            )
            .await
            .expect("cancel");
        let mut delivery = delivery(&store, &world, &world.fake).with_cards(styled(style));
        let report = delivery.drain_notices(now()).await.expect("drain");
        assert_eq!(report.sends.len(), 1, "{style:?}");
        let [post]: [OutgoingMessage; 1] = created(&world.fake).try_into().expect("one post");
        posts.push(post);
    }
    let [classic, redesigned]: [OutgoingMessage; 2] = posts.try_into().expect("two posts");
    assert_eq!(
        classic.content.as_deref(),
        Some("🚫 **HKalos** (Thu 10 Sep) is cancelled — <@1001> Bex\n_(via portal)_")
    );
    assert_eq!(
        redesigned.content.as_deref(),
        Some(
            "🚫 Hard Gatekeeper Kalos is cancelled — <@1001> Bex\n\
             -# Thu 10 Sep 23:00 · via portal"
        )
    );
    assert_eq!(classic.allowed_mentions, redesigned.allowed_mentions);
}
