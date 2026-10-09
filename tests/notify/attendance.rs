//! v5 attendance in dispatch planning: the morning ping names only unknown
//! members, countdowns keep v4 (everyone not declined); v4-compat mode
//! plans exactly v4's mentions for the same schedule.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::domain::attendance::{AttendanceDefault, AttendancePolicy, StandingAnswer};
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::notify::{
    DeliverySettings, DeliveryTarget, DispatchInput, IntentContent, plan_dispatch,
};
use kanade::domain::schedule::{
    FixedRun, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus, ScheduleSnapshot,
};

fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 12, hour, minute, 0).unwrap()
}

fn roster() -> Roster {
    let mut roster = Roster::new();
    for user in ["1001", "1002", "1003", "1004"] {
        roster.upsert(Member {
            user_id: user.into(),
            has_role: true,
            ping_level: PingLevel::All,
            ..Member::default()
        });
    }
    roster
}

/// One run of a weekly timing: 1001 said ✅, 1002 has "always in", 1003
/// never answered, 1004 said ❌; its day-of and 60-minute pings are due.
fn schedule(default: AttendanceDefault) -> ScheduleSnapshot {
    let party: Vec<String> = ["1001", "1002", "1003", "1004"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let answer = |user: &str, state| Rsvp {
        run_id: "r".into(),
        user_id: user.into(),
        state,
        source: RsvpSource::Reaction,
        at: at(0, 0),
    };
    let reminder = |id: &str, kind: &str, fire_at| Reminder {
        id: id.into(),
        run_id: "r".into(),
        kind: kind.into(),
        fire_at,
        sent_at: None,
        message_id: None,
    };
    ScheduleSnapshot {
        revision: 1,
        fixed_runs: vec![FixedRun {
            owner_pinned: false,
            id: "f".into(),
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["Kalos".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: party.clone(),
            note: None,
            attendance_default: default,
            standing: vec![StandingAnswer {
                user_id: "1002".into(),
                set_by: "member:1002".into(),
                at: at(0, 0),
            }],
        }],
        runs: vec![Run {
            id: "r".into(),
            fixed_run_id: Some("f".into()),
            channel_id: Some("222".into()),
            week_start: at(0, 0),
            datetime: at(13, 0),
            bosses: vec!["Kalos".into()],
            participants: party,
            status: RunStatus::AtRisk,
            source: RunSource::Fixed,
            attendance: Vec::new(),
            status_pin: None,
        }],
        reminders: vec![
            reminder("m-day", "day_of", at(1, 0)),
            reminder("m-60", "countdown_60", at(12, 0)),
        ],
        rsvps: vec![
            answer("1001", RsvpState::Yes),
            answer("1004", RsvpState::No),
        ],
        unproven_retired: BTreeSet::new(),
    }
}

/// `(day-of mentions, countdown mentions)`.
fn mentions(
    schedule: &ScheduleSnapshot,
    attendance: AttendancePolicy,
) -> (Vec<String>, Vec<String>) {
    let roster = roster();
    let channels: BTreeSet<String> = ["222".to_owned()].into();
    let journal: BTreeSet<DeliveryTarget> = BTreeSet::new();
    let plan = plan_dispatch(&DispatchInput {
        now: at(12, 1),
        schedule,
        members: &roster,
        channels: &channels,
        journal: &journal,
        settings: DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
            attendance,
        },
    });
    let of = |day_of: bool| {
        plan.sends
            .iter()
            .find(|send| matches!(send.intent.content, IntentContent::DayOf { .. }) == day_of)
            .expect("planned")
            .intent
            .mentions
            .clone()
    };
    (of(true), of(false))
}

#[test]
fn the_morning_ping_names_unknown_members_only_in_v5() {
    let schedule = schedule(AttendanceDefault::OptIn);
    let all = ["1001", "1002", "1003", "1004"];
    let not_declined = ["1001", "1002", "1003"];
    assert_eq!(
        mentions(&schedule, AttendancePolicy::V4_COMPAT),
        (
            all.map(str::to_owned).to_vec(),
            not_declined.map(str::to_owned).to_vec()
        ),
        "v4-compat: v4 exactly"
    );
    assert_eq!(
        mentions(&schedule, AttendancePolicy::V5),
        (
            vec!["1003".to_owned()],
            not_declined.map(str::to_owned).to_vec()
        ),
        "v5: only the unknown member in the morning"
    );
    // An assume-coming timing leaves nobody unknown.
    let assumed = self::schedule(AttendanceDefault::AssumeComing);
    assert!(mentions(&assumed, AttendancePolicy::V5).0.is_empty());
}
