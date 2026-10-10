//! The run-edit steps the admin and member writes share: load the run, turn
//! the client's week `version` into preconditions, write through the one
//! writer and read the run back. Each origin keeps its own checks, refusal
//! codes and projection.

use chrono::{DateTime, Days, TimeZone, Utc};
use serde::Serialize;

use super::{
    precondition::{Explicit, expectations},
    refusal::{Refusal, scheduler},
    runs::strict_time,
    write_context,
};
use crate::{
    api::{
        dto::week::{Context, day_index, run_time},
        error::ApiError,
        state::ApiState,
        write::RunWrite,
    },
    domain::{
        history::{BlameTarget, Origin, rsvp_field},
        members::{Directory, MemberProfile},
        schedule::{RsvpState, Run, RunStatus, SchedulePolicy, ScheduleSnapshot},
        scheduler::{DeclineNoticeContext, SchedulerError, Scope},
    },
};

// Where a moved run was. No doc comment: it would reach the generated TS.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "MovePrevious"))]
pub struct Previous {
    pub day: u8,
    pub time: Option<String>,
}

impl Previous {
    pub fn of(ctx: &Context<'_>, run: &Run) -> Self {
        Self {
            day: day_index(ctx, ctx.local_date(run.week_start), run.datetime),
            time: run_time(ctx, run),
        }
    }
}

/// The one answer for a run that is not there (or not the caller's to see).
pub fn run_not_found() -> Refusal {
    Refusal::new(
        axum::http::StatusCode::NOT_FOUND,
        "not_found",
        "That run no longer exists.",
    )
}

/// The run's snapshot (its reminders and answers); a missing run is a plain
/// 404, never a precondition refusal.
pub async fn load_run(state: &ApiState, run_id: &str) -> Result<ScheduleSnapshot, Refusal> {
    let snapshot = state
        .store
        .snapshot(Scope::Run(run_id.to_owned()))
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    if snapshot.runs.iter().any(|run| run.id == run_id) {
        Ok(snapshot)
    } else {
        Err(run_not_found())
    }
}

/// The run as it stands after a write, with the version read first (so it
/// can only lag the data: a later edit at it may 409, never lose an update).
pub async fn reloaded(state: &ApiState, run_id: &str) -> Result<(ScheduleSnapshot, u64), Refusal> {
    let version = state
        .store
        .head()
        .await
        .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?
        .seq;
    Ok((load_run(state, run_id).await?, version))
}

/// One run edit through the writer, for a run the caller has loaded. A
/// replayed Idempotency-Key is not an error: the caller answers the run as it
/// is now, which is the first attempt's result or later.
pub async fn write_run(
    state: &ApiState,
    origin: Origin,
    run_id: &str,
    fields: &[String],
    (version, explicit): (u64, Explicit),
    write: RunWrite,
) -> Result<Vec<MemberProfile>, Refusal> {
    let expect = expectations(
        state.store.as_ref(),
        BlameTarget::Run(run_id.to_owned()),
        fields,
        Some(version),
        &explicit,
        origin.request_id.is_some(),
    )
    .await?;
    let (ctx, profiles) = write_context(state).await?;
    match state.writer.run(origin, expect, run_id, write, &ctx).await {
        Ok(_) | Err(SchedulerError::AlreadyApplied { .. }) => Ok(profiles),
        Err(error) => Err(scheduler(error)),
    }
}

/// An answer to write: an admin's for anyone (or a clear), or the origin's
/// member's own, which the writer refuses unless the run is theirs to answer.
#[derive(Clone, Copy)]
pub enum Answer {
    Admin(Option<RsvpState>),
    Own(RsvpState),
}

/// `member_id`'s answer on a loaded run, preconditioned on their answer only
/// (other people's edits never make it stale), with the decline notice and
/// its best-effort retraction. Membership is checked by the scheduler after
/// the preconditions.
pub async fn put_answer(
    state: &ApiState,
    origin: Origin,
    run_id: &str,
    member_id: &str,
    answer: Answer,
    version: u64,
    explicit: &Explicit,
) -> Result<Vec<MemberProfile>, Refusal> {
    let expect = expectations(
        state.store.as_ref(),
        BlameTarget::Run(run_id.to_owned()),
        &[rsvp_field(member_id)],
        Some(version),
        explicit,
        origin.request_id.is_some(),
    )
    .await?;
    let (ctx, profiles) = write_context(state).await?;
    let decline = DeclineNoticeContext {
        channel_id: None,
        reference_id: None,
        display_name: ctx
            .directory
            .display_name(member_id)
            .unwrap_or_else(|| member_id.to_owned()),
    };
    let written = match answer {
        Answer::Admin(answer) => {
            state
                .writer
                .rsvp(origin, expect, run_id, member_id, answer, decline)
                .await
        }
        Answer::Own(answer) => {
            state
                .writer
                .member_answer(origin, expect, run_id, answer, decline, &ctx)
                .await
        }
    };
    match written {
        Ok(result) => {
            if result.retract {
                state
                    .retract_decline(run_id.to_owned(), member_id.to_owned())
                    .await;
            }
        }
        Err(SchedulerError::AlreadyApplied { .. }) => {}
        Err(error) => return Err(scheduler(error)),
    }
    Ok(profiles)
}

/// Why a requested slot cannot be built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotError {
    /// Not `HH:MM`.
    Time,
    /// A scheduled run moved without a time.
    NeedsTime,
    /// The local time does not exist that day (a DST gap).
    Missing,
    /// Outside the representable range.
    Range,
    /// It falls in another boss week.
    OtherWeek,
}

/// The instant `day` (0 = the run's reset day) at `time` in the guild zone,
/// inside the run's boss week. Own-time runs may omit the time and keep
/// their clock.
pub fn slot(
    policy: &SchedulePolicy,
    ctx: &Context<'_>,
    run: &Run,
    day: u8,
    time: Option<&str>,
) -> Result<DateTime<Utc>, SlotError> {
    let zone = policy.zone();
    let time = match (time, run.status) {
        (Some(text), _) => strict_time(text).ok_or(SlotError::Time)?,
        (None, RunStatus::Otot) => zone.from_utc_datetime(&run.datetime.naive_utc()).time(),
        (None, _) => return Err(SlotError::NeedsTime),
    };
    let date = ctx.local_date(run.week_start) + Days::new(u64::from(day));
    let to = zone
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .ok_or(SlotError::Missing)?
        .with_timezone(&Utc);
    // With a non-midnight reset, day 0 before the reset time belongs to the
    // previous boss week; a run never leaves its week.
    let week = policy.week_of(&to).map_err(|_| SlotError::Range)?;
    if week.to_fixed() == run.week_start {
        Ok(to)
    } else {
        Err(SlotError::OtherWeek)
    }
}
