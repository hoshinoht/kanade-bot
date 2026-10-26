//! The day-of, countdown and digest cards over one fixed boss week, and
//! change notices.

mod chart;
mod live;
pub(crate) mod notices;
mod portal;
mod styled;

use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use kanade::bot::delivery::cards::CardContext;
use kanade::bot::delivery::cards::redesign::{DifficultyMarks, NO_MARKS};
use kanade::domain::art::{ArtFile, ArtSource};
use kanade::domain::catalog::BossTable;
use kanade::domain::guides::ArtKind;
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::notify::{DigestInclusion, IntentContent, digest_inclusion};
use kanade::domain::schedule::{
    Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus, ScheduleSnapshot,
};

pub(crate) const ZONE: Tz = chrono_tz::Asia::Kuala_Lumpur;
pub(crate) const MALEFIC: &str = "a1b2c3d4-0000-4000-8000-000000000001";
pub(crate) const KALOS: &str = "e5f6a7b8-0000-4000-8000-000000000002";
pub(crate) const CLEARED: &str = "c1d2e3f4-0000-4000-8000-000000000003";
pub(crate) const RISKY: &str = "f1e2d3c4-0000-4000-8000-000000000004";
pub(crate) const OWN_TIME: &str = "b0b0b0b0-0000-4000-8000-000000000005";
pub(crate) const CANCELLED: &str = "ab12ab12-0000-4000-8000-000000000006";

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0).unwrap()
}

/// Thu 03 Sep 00:00 in the guild zone.
pub(crate) fn week_start() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 2, 16, 0, 0).unwrap()
}

fn run(
    id: &str,
    channel: &str,
    at: DateTime<Utc>,
    bosses: &[&str],
    party: &[&str],
    status: RunStatus,
) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some(channel.into()),
        week_start: week_start(),
        datetime: at,
        bosses: bosses.iter().map(|boss| (*boss).to_owned()).collect(),
        participants: party.iter().map(|user| (*user).to_owned()).collect(),
        status,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn rsvp(run_id: &str, user: &str, state: RsvpState) -> Rsvp {
    Rsvp {
        run_id: run_id.into(),
        user_id: user.into(),
        state,
        source: RsvpSource::Reaction,
        at: utc(1, 0, 0),
    }
}

/// The sample week: two runs on Wed 09 Sep (one waiting, one all in), a
/// cleared Saturday run in another channel, an at-risk Tuesday, an own-time
/// run with an unknown boss and a cancelled one.
pub(crate) fn week() -> ScheduleSnapshot {
    let party = ["1001", "1002", "1003"];
    ScheduleSnapshot {
        runs: vec![
            run(
                CANCELLED,
                "111",
                utc(4, 13, 0),
                &["HKalos"],
                &party,
                RunStatus::Cancelled,
            ),
            run(
                CLEARED,
                "222",
                utc(5, 13, 0),
                &["HKalos"],
                &["1001", "1002"],
                RunStatus::Done,
            ),
            run(
                OWN_TIME,
                "111",
                utc(7, 12, 0),
                &["XFoo"],
                &["1003"],
                RunStatus::Otot,
            ),
            run(
                RISKY,
                "111",
                utc(8, 14, 0),
                &["NMaleficStar"],
                &party,
                RunStatus::AtRisk,
            ),
            run(
                MALEFIC,
                "111",
                utc(9, 13, 30),
                &["HMaleficStar", "HKalos"],
                &party,
                RunStatus::Planned,
            ),
            run(
                KALOS,
                "111",
                utc(9, 15, 0),
                &["XKalos"],
                &["1001", "1003"],
                RunStatus::Confirmed,
            ),
        ],
        rsvps: vec![
            rsvp(CLEARED, "1001", RsvpState::Yes),
            rsvp(CLEARED, "1002", RsvpState::Yes),
            rsvp(RISKY, "1002", RsvpState::No),
            rsvp(MALEFIC, "1001", RsvpState::Yes),
            rsvp(MALEFIC, "1002", RsvpState::No),
            rsvp(KALOS, "1001", RsvpState::Yes),
            rsvp(KALOS, "1003", RsvpState::Yes),
        ],
        ..ScheduleSnapshot::default()
    }
}

/// 1001 "Aria", 1002 "Bex", 1003 nameless.
pub(crate) fn members() -> Roster {
    let mut roster = Roster::new();
    for (user, name) in [
        ("1001", Some("Aria")),
        ("1002", Some("Bex")),
        ("1003", None),
    ] {
        roster.upsert(Member {
            user_id: user.into(),
            display_name: name.map(str::to_owned),
            has_role: true,
            ping_level: PingLevel::All,
            ..Member::default()
        });
    }
    roster
}

pub(crate) fn inclusion(schedule: &ScheduleSnapshot) -> DigestInclusion {
    digest_inclusion(&schedule.runs, week_start(), ZONE, None).expect("inclusion")
}

pub(crate) fn context<'a>(
    schedule: &'a ScheduleSnapshot,
    members: &'a Roster,
    catalog: &'a BossTable,
    quiet: bool,
) -> CardContext<'a> {
    CardContext {
        schedule,
        zone: ZONE,
        quiet,
        members,
        catalog: Some(catalog),
        marks: &NO_MARKS,
        v2: None,
    }
}

/// [`context`] with `marks`.
pub(crate) fn redesigned<'a>(
    schedule: &'a ScheduleSnapshot,
    members: &'a Roster,
    catalog: &'a BossTable,
    quiet: bool,
    marks: &'a DifficultyMarks,
) -> CardContext<'a> {
    CardContext {
        marks,
        ..context(schedule, members, catalog, quiet)
    }
}

/// [`redesigned`], not quiet, with no difficulty marks.
pub(crate) fn redesigned_v5<'a>(
    schedule: &'a ScheduleSnapshot,
    members: &'a Roster,
    catalog: &'a BossTable,
) -> CardContext<'a> {
    redesigned(schedule, members, catalog, false, &NO_MARKS)
}

/// Replace the week's runs with `count` copies of its Extreme Kalos run ten
/// minutes apart, each adjusted by `edit`; returns their ids.
pub(crate) fn kalos_copies(
    schedule: &mut ScheduleSnapshot,
    count: i64,
    edit: impl Fn(&mut Run),
) -> Vec<String> {
    let base = schedule
        .runs
        .iter()
        .find(|run| run.id == KALOS)
        .unwrap()
        .clone();
    schedule.runs = (0..count)
        .map(|index| {
            let mut run = Run {
                id: format!("{index:08x}-0000-4000-8000-000000000000"),
                datetime: base.datetime + TimeDelta::minutes(10 * index),
                ..base.clone()
            };
            edit(&mut run);
            run
        })
        .collect();
    schedule.runs.iter().map(|run| run.id.clone()).collect()
}

pub(crate) fn day_of(run_ids: &[&str]) -> IntentContent {
    IntentContent::DayOf {
        run_ids: run_ids.iter().map(|id| (*id).to_owned()).collect(),
    }
}

pub(crate) fn countdown(run_id: &str) -> IntentContent {
    IntentContent::Countdown {
        run_id: run_id.into(),
        minutes: 15,
    }
}

/// Every picture exists: `<basename>.png`, bytes naming it.
pub(crate) struct AllArt;

impl ArtSource for AllArt {
    fn find(&self, kind: ArtKind, basename: &str, read: bool) -> Option<ArtFile> {
        Some(ArtFile {
            file_name: format!("{basename}.png"),
            bytes: read.then(|| format!("{kind:?}:{basename}").into_bytes()),
        })
    }
}

pub(crate) fn art() -> Arc<dyn ArtSource> {
    Arc::new(AllArt)
}
