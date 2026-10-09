//! Wire shapes of the member session routes (`public.json`). The structs
//! carry no doc comments so the generated TypeScript stays exactly the
//! frozen shapes; the schema documents each field.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use super::{
    Boss, iso_date, iso_instant,
    limits::{Allowance, Quota},
    week::{
        Context, Model, Participant, Tally, WeekDay, WeekFrame, day_index, days, participants,
        run_time, tally,
    },
};
use crate::{
    domain::{
        schedule::{Run, ScheduleSnapshot},
        settings::RunLengths,
    },
    infrastructure::llm::governor::{CallKind, GroupSnapshot},
};

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicMember {
    pub id: String,
    pub display: String,
    pub avatar: String,
}

// The CSRF token travels in `X-Kanade-CSRF`, never in the body.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicSession {
    pub member: PublicMember,
    pub fresh_until: String,
}

// No address or location: only what the device list shows (D5-A).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicSessionRow {
    pub handle: String,
    pub device: Option<String>,
    pub signed_in_at: String,
    pub last_seen_at: String,
    pub current: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct PublicSessions {
    pub sessions: Vec<PublicSessionRow>,
    pub generated_at: String,
}

// `admin-api` `Run` minus `short_id`, `channel_id`, `cards`, `amended` and
// `roster_change`; names and answers on every run (Q3).
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberRun {
    pub id: String,
    pub day: u8,
    pub time: Option<String>,
    pub minutes: u32,
    #[cfg_attr(test, ts(type = "RunStatus"))]
    pub status: &'static str,
    pub bosses: Vec<Boss>,
    pub tally: Tally,
    pub participants: Vec<Participant>,
    pub party: String,
    pub channel: String,
    pub fixed_id: Option<String>,
    pub mine: bool,
    pub can_edit: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberWeek {
    pub starts: String,
    pub timezone: String,
    pub reset: String,
    pub days: Vec<WeekDay>,
    pub runs: Vec<MemberRun>,
    pub generated_at: String,
    pub version: u64,
}

// The caller's own chat allowance and a coarse bot status; never another
// member's usage or the queue's other entries.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberAllowance {
    pub allowance: Option<Quota>,
    pub used: usize,
    pub resets_at: Option<String>,
    pub queue_position: Option<usize>,
    pub bot_busy: bool,
    pub generated_at: String,
}

/// One run as `user_id` sees it: the admin run's shared fields plus whether
/// it is theirs. Only this and next boss week are ever projected, so
/// `can_edit` needs no week check of its own.
pub fn member_run(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    start_date: NaiveDate,
    run: &Run,
    run_lengths: &RunLengths,
    user_id: &str,
) -> MemberRun {
    let participants = participants(ctx, snapshot, run);
    let channel = ctx.channel_name(run.channel_id.as_deref().unwrap_or_default());
    let mine = run.participants.iter().any(|id| id == user_id);
    MemberRun {
        id: run.id.clone(),
        day: day_index(ctx, start_date, run.datetime),
        time: run_time(ctx, run),
        minutes: run_lengths.minutes_for(ctx.catalog, &run.bosses),
        status: run.status.as_str(),
        bosses: ctx.bosses(&run.bosses),
        tally: tally(&participants),
        participants,
        party: channel.clone(),
        channel,
        fixed_id: run.fixed_run_id.clone(),
        mine,
        can_edit: mine && !run.status.is_terminal(),
    }
}

/// The admin `Week` frame and run order, each run seen by `user_id`.
pub fn member_week(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    frame: &WeekFrame,
    version: u64,
    run_lengths: &RunLengths,
    user_id: &str,
) -> MemberWeek {
    let start_date = ctx.local_date(frame.start);
    MemberWeek {
        starts: iso_date(start_date),
        timezone: ctx.zone.name().to_owned(),
        reset: frame.reset.clone(),
        days: days(ctx, start_date),
        runs: snapshot
            .runs
            .iter()
            .filter(|run| run.week_start == frame.start)
            .map(|run| member_run(ctx, snapshot, start_date, run, run_lengths, user_id))
            .collect(),
        generated_at: iso_instant(ctx.now),
        version,
    }
}

/// `row` is the caller's Limits allowance row; `None` means no chatbot access,
/// shown as an allowance of nothing over the default window (`null` would
/// read as "no limit"). The queue entry is the caller's own chat call only.
pub fn member_allowance(
    row: Option<Allowance>,
    default_window_s: f64,
    user_id: &str,
    groups: &[GroupSnapshot],
    now: DateTime<Utc>,
) -> MemberAllowance {
    let (allowance, used, resets_at) = match row {
        Some(row) => (row.allowance, row.used, row.resets_at),
        None => (
            Some(Quota {
                count: 0,
                per_s: default_window_s,
            }),
            0,
            None,
        ),
    };
    MemberAllowance {
        allowance,
        used,
        resets_at,
        queue_position: groups
            .iter()
            .flat_map(|group| &group.queue)
            .find(|call| call.kind == CallKind::Chat && call.who == user_id)
            .map(|call| call.position as usize),
        bot_busy: Model::from_groups(groups).busy,
        generated_at: iso_instant(now),
    }
}
