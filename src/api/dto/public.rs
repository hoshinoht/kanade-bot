//! Wire shapes of the member session routes (`public.json`). The structs
//! carry no doc comments so the generated TypeScript stays exactly the
//! frozen shapes; the schema documents each field. The member's own-run
//! writes and requests are in `runs` and `requests`.

mod requests;
mod runs;

pub use requests::{
    MemberProposed, MemberRequest, MemberRequestLimit, MemberRequestOptions, MemberRequests,
    RequestView,
};
pub use runs::{
    MemberMoveResult, MemberRemoved, MemberRunLink, MemberRunResult, MemberTimingSlot, RunWeek,
};

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use super::{
    Boss, Named, hhmm, iso_date, iso_instant,
    limits::{Allowance, Quota},
    week::{
        Context, Model, Participant, Tally, WeekDay, WeekFrame, day_index, days, participants,
        run_time, tally,
    },
};
use crate::{
    domain::{
        completion::RunEnds,
        history::Actor,
        ownership::OwnerRequest,
        schedule::{FixedRun, Run, ScheduleSnapshot},
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
/// it is theirs. `can_edit` assumes this or next boss week; runs from other
/// weeks go through [`member_run_in`]. A live run past its end is frozen
/// until it is settled, so it is not editable either.
pub fn member_run(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    start_date: NaiveDate,
    run: &Run,
    ends: &RunEnds,
    user_id: &str,
) -> MemberRun {
    let participants = participants(ctx, snapshot, run);
    let channel = run
        .channel_id
        .as_deref()
        .map_or_else(String::new, |id| ctx.member_channel_label(id));
    let mine = run.participants.iter().any(|id| id == user_id);
    MemberRun {
        id: run.id.clone(),
        day: day_index(ctx, start_date, run.datetime),
        time: run_time(ctx, run),
        minutes: ends.minutes(&run.bosses),
        status: run.status.as_str(),
        bosses: ctx.bosses(&run.bosses),
        tally: tally(&participants),
        participants,
        party: channel.clone(),
        channel,
        fixed_id: run.fixed_run_id.clone(),
        mine,
        can_edit: mine && !run.status.is_terminal() && !ends.frozen(run, ctx.now),
    }
}

/// [`member_run`] for a run of any boss week: editable only in this or next.
pub fn member_run_in(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    run: &Run,
    week: RunWeek,
    ends: &RunEnds,
    user_id: &str,
) -> MemberRun {
    let start = ctx.local_date(run.week_start);
    let mut view = member_run(ctx, snapshot, start, run, ends, user_id);
    view.can_edit &= week.open();
    view
}

/// Who made a change, as members may see it: a member's name, never an
/// administrator's or a component's label.
pub fn actor_label(ctx: &Context<'_>, actor: &Actor) -> String {
    match actor {
        Actor::Member { id } => ctx.name(id),
        Actor::Admin { .. } => "an admin".to_owned(),
        Actor::System { .. } => "Kanade".to_owned(),
    }
}

/// The admin `Week` frame and run order, each run seen by `user_id`.
pub fn member_week(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    frame: &WeekFrame,
    version: u64,
    ends: &RunEnds,
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
            .map(|run| member_run(ctx, snapshot, start_date, run, ends, user_id))
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

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberOwnerRequest {
    pub id: String,
    pub requester: Named,
    pub created_at: String,
    pub expires_at: String,
    #[cfg_attr(
        test,
        ts(type = "'open' | 'accepted' | 'declined' | 'expired' | 'withdrawn' | 'superseded'")
    )]
    pub status: &'static str,
    pub mine: bool,
}

// A weekly timing the member is on, with its ownership; never the channel,
// note or another member's request detail unless the member owns it.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberTiming {
    pub id: String,
    pub bosses: Vec<Boss>,
    pub weekday: u32,
    pub time: String,
    pub party: Vec<Named>,
    pub owner: Named,
    pub owner_pinned: bool,
    pub you_own: bool,
    pub requests: Vec<MemberOwnerRequest>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct MemberTimings {
    pub timings: Vec<MemberTiming>,
    pub generated_at: String,
}

pub fn member_owner_request(
    ctx: &Context<'_>,
    request: &OwnerRequest,
    user_id: &str,
) -> MemberOwnerRequest {
    MemberOwnerRequest {
        id: request.id.clone(),
        requester: ctx.named(&request.requester),
        created_at: iso_instant(request.created_at),
        expires_at: iso_instant(request.expires_at),
        status: request.status.as_str(),
        mine: request.requester == user_id,
    }
}

/// `fixed` as `user_id` sees it. `open` may hold any open requests: only
/// this timing's unexpired ones are kept, all of them for its owner and
/// otherwise only the member's own.
pub fn member_timing(
    ctx: &Context<'_>,
    fixed: &FixedRun,
    open: &[OwnerRequest],
    user_id: &str,
) -> MemberTiming {
    let you_own = fixed.owner() == user_id;
    MemberTiming {
        id: fixed.id.clone(),
        bosses: ctx.bosses(&fixed.bosses),
        weekday: fixed.weekday.num_days_from_monday(),
        time: hhmm(fixed.time),
        party: fixed.participants.iter().map(|id| ctx.named(id)).collect(),
        owner: ctx.named(fixed.owner()),
        owner_pinned: fixed.owner_pinned,
        you_own,
        requests: open
            .iter()
            .filter(|request| {
                request.fixed_run_id == fixed.id
                    && request.live(ctx.now)
                    && (you_own || request.requester == user_id)
            })
            .map(|request| member_owner_request(ctx, request, user_id))
            .collect(),
    }
}

/// The weekly timings `user_id` is on (party only).
pub fn member_timings(
    ctx: &Context<'_>,
    fixed_runs: &[FixedRun],
    open: &[OwnerRequest],
    user_id: &str,
) -> MemberTimings {
    MemberTimings {
        timings: fixed_runs
            .iter()
            .filter(|fixed| fixed.participants.iter().any(|id| id == user_id))
            .map(|fixed| member_timing(ctx, fixed, open, user_id))
            .collect(),
        generated_at: iso_instant(ctx.now),
    }
}
