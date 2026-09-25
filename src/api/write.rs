//! The API's single scheduler writer: one `SchedulerService` behind an async
//! mutex, so admin mutations are serialised; reads never take it. The port is
//! object-safe so `ApiState` stays non-generic.

use std::{future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;

use super::auth::Clock as ApiClockFn;
use crate::domain::{
    history::{Expect, Origin},
    members::Roster,
    schedule::{FixedEditRequest, NewFixedRun, RsvpState, SchedulePolicy, StatusChange},
    scheduler::{Clock, IdSource, ScheduleStore, SchedulerResult, SchedulerService},
};

pub type WriteFuture<'a, T> = Pin<Box<dyn Future<Output = SchedulerResult<T>> + Send + 'a>>;

/// What every write reads besides its arguments.
pub struct WriteContext {
    pub policy: SchedulePolicy,
    /// Members and watched channels, for participant and channel checks.
    pub directory: Roster,
}

/// One run edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunWrite {
    Move {
        to: DateTime<Utc>,
    },
    Status(StatusChange),
    /// A portal answer (source `chat`, status re-derived, a status pin kept);
    /// `None` clears whatever the member answered.
    Rsvp {
        user_id: String,
        answer: Option<RsvpState>,
    },
    Participants {
        add: Vec<String>,
        remove: Vec<String>,
    },
    Reset,
}

pub trait Writer: Send + Sync {
    fn run<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        write: RunWrite,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()>;

    fn add_fixed<'a>(
        &'a self,
        origin: Origin,
        new: NewFixedRun,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, String>;

    fn edit_fixed<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        request: FixedEditRequest,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()>;

    /// Cancels the timing's live runs in the materialised weeks.
    fn retire_fixed<'a>(
        &'a self,
        origin: Origin,
        fixed_id: &'a str,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, usize>;

    /// Materialise the current and next two boss weeks (a new timing's runs).
    fn materialise<'a>(
        &'a self,
        origin: Origin,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, Vec<String>>;
}

/// The scheduler clock over the API's pinned-or-system clock.
pub struct ApiClock(pub ApiClockFn);

impl Clock for ApiClock {
    fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}

pub struct SchedulerWriter<S, I, C> {
    service: Mutex<SchedulerService<S, I, C>>,
}

impl<S, I, C> SchedulerWriter<S, I, C> {
    pub fn new(service: SchedulerService<S, I, C>) -> Self {
        Self {
            service: Mutex::new(service),
        }
    }
}

impl<S, I, C> Writer for SchedulerWriter<S, I, C>
where
    S: ScheduleStore + Send + Sync,
    I: IdSource + Send,
    C: Clock + Send + Sync,
{
    fn run<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        run_id: &'a str,
        write: RunWrite,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            let handle = service.as_origin(origin).expecting(expect);
            match write {
                RunWrite::Move { to } => handle.amend_run(run_id, to, &ctx.policy).await.map(drop),
                RunWrite::Status(change) => handle
                    .set_status(run_id, change, &ctx.policy.reminders)
                    .await
                    .map(drop),
                RunWrite::Rsvp { user_id, answer } => handle
                    .portal_answer(run_id, &user_id, answer)
                    .await
                    .map(drop),
                RunWrite::Participants { add, remove } => handle
                    .swap_participants(run_id, &remove, &add, true, &ctx.directory)
                    .await
                    .map(drop),
                RunWrite::Reset => handle.reset_to_fixed(run_id, &ctx.policy).await.map(drop),
            }
        })
    }

    fn add_fixed<'a>(
        &'a self,
        origin: Origin,
        new: NewFixedRun,
        _: &'a WriteContext,
    ) -> WriteFuture<'a, String> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service.as_origin(origin).add_fixed_run(new).await
        })
    }

    fn edit_fixed<'a>(
        &'a self,
        origin: Origin,
        expect: Expect,
        request: FixedEditRequest,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, ()> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .expecting(expect)
                .apply_fixed_edit(&request, &ctx.directory, &ctx.policy)
                .await
                .map(|_| ())
        })
    }

    fn retire_fixed<'a>(
        &'a self,
        origin: Origin,
        fixed_id: &'a str,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, usize> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            let now = service.clock().now();
            let weeks = ctx
                .policy
                .materialised_weeks(now)
                .map_err(crate::domain::schedule::ScheduleError::from)?;
            service
                .as_origin(origin)
                .retire_fixed_run(fixed_id, &weeks, &ctx.policy.reminders)
                .await
        })
    }

    fn materialise<'a>(
        &'a self,
        origin: Origin,
        ctx: &'a WriteContext,
    ) -> WriteFuture<'a, Vec<String>> {
        Box::pin(async move {
            let mut service = self.service.lock().await;
            service
                .as_origin(origin)
                .materialise_weeks(&ctx.policy)
                .await
        })
    }
}

/// A shared writer.
pub type SharedWriter = Arc<dyn Writer>;
