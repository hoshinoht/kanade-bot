//! Message style: the classic cards pinned byte-exact, the redesigned
//! day-of, countdown and digest cards over one fixed boss week, and change
//! notices in both styles.

mod classic;
mod live;
pub(crate) mod notices;
mod styled;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use kanade::bot::delivery::cards::redesign::{DifficultyMarks, NO_MARKS};
use kanade::bot::delivery::cards::{ArtFile, ArtKind, ArtSource, CardContext};
use kanade::bot::transport::{MessageEdit, OutgoingMessage};
use kanade::domain::attendance::AttendancePolicy;
use kanade::domain::catalog::BossTable;
use kanade::domain::members::{Member, PingLevel, Roster};
use kanade::domain::notify::{DigestInclusion, digest_inclusion};
use kanade::domain::schedule::{
    Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus, ScheduleSnapshot,
};
use kanade::domain::settings::MessageStyle;
use serde_json::{Value, json};

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
    digest_inclusion(&schedule.runs, week_start(), ZONE).expect("inclusion")
}

pub(crate) fn context<'a>(
    schedule: &'a ScheduleSnapshot,
    members: &'a Roster,
    catalog: &'a BossTable,
    attendance: AttendancePolicy,
    quiet: bool,
) -> CardContext<'a> {
    CardContext {
        schedule,
        attendance,
        zone: ZONE,
        quiet,
        members,
        catalog: Some(catalog),
        style: MessageStyle::Classic,
        marks: &NO_MARKS,
        v2: None,
    }
}

/// [`context`] in the redesigned style with `marks`.
pub(crate) fn redesigned<'a>(
    schedule: &'a ScheduleSnapshot,
    members: &'a Roster,
    catalog: &'a BossTable,
    attendance: AttendancePolicy,
    quiet: bool,
    marks: &'a DifficultyMarks,
) -> CardContext<'a> {
    CardContext {
        style: MessageStyle::Redesigned,
        marks,
        ..context(schedule, members, catalog, attendance, quiet)
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

/// A post as JSON: content, the embeds as Discord receives them, uploads
/// (name and bytes) and the allow-list.
pub(crate) fn message_json(message: &OutgoingMessage) -> Value {
    json!({
        "content": message.content,
        "embeds": message.embeds,
        "attachments": message
            .attachments
            .iter()
            .map(|upload| json!([upload.filename, String::from_utf8_lossy(&upload.bytes)]))
            .collect::<Vec<_>>(),
        "allowed_mentions": message.allowed_mentions,
    })
}

pub(crate) fn edit_json(edit: &MessageEdit) -> Value {
    json!({
        "content": edit.content,
        "embeds": edit.embeds,
        "allowed_mentions": edit.allowed_mentions,
    })
}
