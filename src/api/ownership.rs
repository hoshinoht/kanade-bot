//! Weekly-timing ownership actions shared by Discord and the portal (user
//! decision 2026-10-10): hand off, ask, accept, decline, withdraw and expire.
//! Rules are `domain::ownership`; every owner change is a recorded fixed
//! edit pinning the new owner, keyed by the action so a retry replays it.

use chrono::{DateTime, Utc};

use crate::api::state::ReadStore;
use crate::api::write::{WriteContext, Writer};
use crate::domain::history::{ChangeRecord, Origin};
use crate::domain::ownership::{
    self, OwnerPin, OwnerRequest, OwnerRequestStatus, OwnershipRefusal,
};
use crate::domain::schedule::{FixedRun, ScheduleError};
use crate::domain::scheduler::{SchedulerError, Scope, StoreError};

#[derive(Debug)]
pub enum OwnershipError {
    Refused(OwnershipRefusal),
    UnknownTiming,
    UnknownRequest,
    /// The member already has an open request on this timing.
    AlreadyAsked,
    Scheduler(SchedulerError),
    Store(StoreError),
}

impl From<OwnershipRefusal> for OwnershipError {
    fn from(refusal: OwnershipRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<StoreError> for OwnershipError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl std::fmt::Display for OwnershipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(f),
            Self::UnknownTiming => f.write_str("No such weekly timing."),
            Self::UnknownRequest => f.write_str("No such ownership request."),
            Self::AlreadyAsked => f.write_str("You already asked to own this timing."),
            Self::Scheduler(error) => error.fmt(f),
            Self::Store(_) => f.write_str("The schedule is unavailable; try again."),
        }
    }
}

/// What an action changed, for the caller's messages.
#[derive(Debug)]
pub struct OwnerChange {
    /// The timing after the action.
    pub fixed: FixedRun,
    /// Other open requests on the timing that its new owner closed.
    pub superseded: Vec<OwnerRequest>,
}

async fn timing(store: &dyn ReadStore, fixed_id: &str) -> Result<FixedRun, OwnershipError> {
    store
        .snapshot(Scope::Weeks(Vec::new()))
        .await?
        .fixed_runs
        .into_iter()
        .find(|fixed| fixed.id == fixed_id)
        .ok_or(OwnershipError::UnknownTiming)
}

/// The change `origin`'s actor recorded under its request id on the same
/// surface, if any: only then is the action a retry that skips the rules it
/// passed the first time (a Discord staff decision is never finished as
/// staff from the portal).
async fn recorded(
    store: &dyn ReadStore,
    origin: &Origin,
) -> Result<Option<ChangeRecord>, OwnershipError> {
    let Some(request_id) = origin.request_id.clone() else {
        return Ok(None);
    };
    Ok(store
        .recorded_change(origin.actor.clone(), request_id)
        .await?
        .filter(|record| record.origin.surface == origin.surface))
}

/// The request id an accept pins under, shared by every surface.
fn accept_key(request: &OwnerRequest) -> String {
    format!("owner-request:{}", request.id)
}

/// Pin `change`'s new owner with a recorded edit, `change` re-checked on the
/// committed timing. `true` when this call committed it; `false` when
/// `origin`'s request id had already recorded it (a replay).
async fn pin(
    writer: &dyn Writer,
    origin: Origin,
    fixed_id: &str,
    change: OwnerPin<'_>,
    ctx: &WriteContext,
) -> Result<bool, OwnershipError> {
    match writer.pin_owner(origin, fixed_id, change, ctx).await {
        Ok(()) => Ok(true),
        Err(SchedulerError::AlreadyApplied { .. }) => Ok(false),
        Err(SchedulerError::Schedule(ScheduleError::Ownership(refusal))) => Err(refusal.into()),
        Err(SchedulerError::Schedule(ScheduleError::UnknownFixedRun(_))) => {
            Err(OwnershipError::UnknownTiming)
        }
        Err(error) => Err(OwnershipError::Scheduler(error)),
    }
}

/// Close every other open request on the timing (`keep` aside) opened
/// before `before`, when given: its owner changed.
async fn supersede(
    store: &dyn ReadStore,
    fixed_id: &str,
    keep: Option<&str>,
    before: Option<DateTime<Utc>>,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<Vec<OwnerRequest>, OwnershipError> {
    let mut closed = Vec::new();
    for request in store.open_owner_requests().await? {
        if request.fixed_run_id != fixed_id
            || Some(request.id.as_str()) == keep
            || before.is_some_and(|before| request.created_at >= before)
        {
            continue;
        }
        let id = request.id.clone();
        if store
            .close_owner_request(id, OwnerRequestStatus::Superseded, actor.to_owned(), now)
            .await?
        {
            closed.push(request);
        }
    }
    Ok(closed)
}

/// A replay finishes only what its first attempt may have left: the asks
/// opened before its recorded pin, and only while the owner it pinned still
/// owns the timing (a later owner's asks are theirs to answer).
async fn leftover(
    store: &dyn ReadStore,
    fixed: &FixedRun,
    pinned: &str,
    record: Option<ChangeRecord>,
    keep: Option<&str>,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<Vec<OwnerRequest>, OwnershipError> {
    match record {
        Some(record) if fixed.owner() == pinned => {
            supersede(store, &fixed.id, keep, Some(record.at), actor, now).await
        }
        _ => Ok(Vec::new()),
    }
}

#[cfg(any(test, feature = "test-support"))]
type Race = std::pin::Pin<Box<dyn Future<Output = ()> + Send>>;

/// Pending [`after_accept_pin`] races, by request id.
#[cfg(any(test, feature = "test-support"))]
static RACES: std::sync::Mutex<Vec<(String, Race)>> = std::sync::Mutex::new(Vec::new());

/// Test-only: run `race` once, right after an accept of `request_id`
/// commits its pin and before it closes the request (a withdraw landing in
/// between, say).
#[cfg(any(test, feature = "test-support"))]
pub fn after_accept_pin(
    request_id: impl Into<String>,
    race: impl Future<Output = ()> + Send + 'static,
) {
    RACES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((request_id.into(), Box::pin(race)));
}

#[cfg(any(test, feature = "test-support"))]
async fn raced(request_id: &str) {
    let race = {
        let mut races = RACES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        races
            .iter()
            .position(|(id, _)| id == request_id)
            .map(|at| races.remove(at).1)
    };
    if let Some(race) = race {
        race.await;
    }
}

/// Acting user as the store records deciders: `member:<id>` or the admin id.
pub fn actor(origin: &Origin) -> String {
    format!("{}:{}", origin.actor.kind(), origin.actor.id())
}

/// A party member asks to own the timing.
///
/// # Errors
/// [`OwnershipError`]; [`OwnershipError::AlreadyAsked`] for a second open one.
pub async fn ask(
    store: &dyn ReadStore,
    fixed_id: &str,
    requester: &str,
    id: String,
    now: DateTime<Utc>,
) -> Result<(FixedRun, OwnerRequest), OwnershipError> {
    let fixed = timing(store, fixed_id).await?;
    ownership::may_request(&fixed, requester)?;
    let request = OwnerRequest::open(
        id,
        fixed.id.clone(),
        requester.to_owned(),
        fixed.channel_id.clone(),
        now,
    );
    match store.create_owner_request(request.clone()).await {
        Ok(()) => Ok((fixed, request)),
        Err(StoreError::Constraint(_)) => Err(OwnershipError::AlreadyAsked),
        Err(error) => Err(error.into()),
    }
}

/// The store and writer every owner change goes through.
pub struct OwnerDesk<'a> {
    pub store: &'a dyn ReadStore,
    pub writer: &'a dyn Writer,
    pub ctx: &'a WriteContext,
}

impl OwnerDesk<'_> {
    /// The owner (or staff) hands the timing to `to`, another party member.
    /// `origin` carries the action's request id: only the caller's own
    /// recorded id on the same surface replays, finishing only what the
    /// first attempt left ([`leftover`]); anything else is judged here and
    /// again in the commit.
    ///
    /// # Errors
    /// [`OwnershipError`].
    pub async fn hand_off(
        &self,
        origin: Origin,
        fixed_id: &str,
        giver: &str,
        staff: bool,
        to: &str,
        now: DateTime<Utc>,
    ) -> Result<OwnerChange, OwnershipError> {
        let Self { store, writer, ctx } = *self;
        let mut record = recorded(store, &origin).await?;
        if record.is_none() {
            ownership::hand_off(&timing(store, fixed_id).await?, giver, staff, to)?;
        }
        let who = actor(&origin);
        let change = OwnerPin::HandOff { giver, staff, to };
        let committed = pin(writer, origin.clone(), fixed_id, change, ctx).await?;
        let fixed = timing(store, fixed_id).await?;
        let superseded = if committed {
            supersede(store, fixed_id, None, None, &who, now).await?
        } else {
            // A same-key retry that raced the first attempt into the store.
            if record.is_none() {
                record = recorded(store, &origin).await?;
            }
            leftover(store, &fixed, to, record, None, &who, now).await?
        };
        Ok(OwnerChange { fixed, superseded })
    }

    /// The owner (or staff) accepts or declines a request. Accepting pins the
    /// requester under [`accept_key`] and supersedes the timing's other open
    /// requests. The decider's own retry on the same surface (the pin landed,
    /// the close did not) only closes the accepted request itself. A
    /// committed pin whose close lost to a withdraw still counts: the owner
    /// changed, and the request is answered as it now is.
    ///
    /// # Errors
    /// [`OwnershipError`].
    pub async fn decide(
        &self,
        origin: Origin,
        request_id: &str,
        decider: &str,
        staff: bool,
        accept: bool,
        now: DateTime<Utc>,
    ) -> Result<(OwnerRequest, OwnerChange), OwnershipError> {
        let Self { store, writer, ctx } = *self;
        let request = store
            .owner_request(request_id.to_owned())
            .await?
            .ok_or(OwnershipError::UnknownRequest)?;
        let fixed = timing(store, &request.fixed_run_id).await?;
        let who = actor(&origin);
        let (status, committed) = if accept {
            let origin = origin.with_request_id(accept_key(&request));
            if recorded(store, &origin).await?.is_none() {
                ownership::may_decide(&fixed, &request, decider, staff, true, now)?;
            }
            let change = OwnerPin::Accept {
                decider,
                staff,
                requester: &request.requester,
            };
            let committed = pin(writer, origin, &fixed.id, change, ctx).await?;
            #[cfg(any(test, feature = "test-support"))]
            if committed {
                raced(&request.id).await;
            }
            (OwnerRequestStatus::Accepted, committed)
        } else {
            ownership::may_decide(&fixed, &request, decider, staff, false, now)?;
            (OwnerRequestStatus::Declined, false)
        };
        let closed = store
            .close_owner_request(request.id.clone(), status, who.clone(), now)
            .await?;
        if !closed && !committed {
            return Err(OwnershipRefusal::Closed.into());
        }
        let superseded = if committed {
            supersede(store, &fixed.id, Some(&request.id), None, &who, now).await?
        } else {
            Vec::new()
        };
        let decided = store
            .owner_request(request.id.clone())
            .await?
            .ok_or(OwnershipError::UnknownRequest)?;
        Ok((
            decided,
            OwnerChange {
                fixed: timing(store, &fixed.id).await?,
                superseded,
            },
        ))
    }

    /// The decider's retry of an accept that already closed `request`:
    /// close what its supersede may have missed, by the replay rule
    /// ([`leftover`]). Nothing when `origin` recorded no pin for it.
    ///
    /// # Errors
    /// [`OwnershipError`].
    pub async fn finish_accepted(
        &self,
        origin: Origin,
        request: &OwnerRequest,
        now: DateTime<Utc>,
    ) -> Result<Vec<OwnerRequest>, OwnershipError> {
        let origin = origin.with_request_id(accept_key(request));
        let record = recorded(self.store, &origin).await?;
        let fixed = timing(self.store, &request.fixed_run_id).await?;
        let keep = Some(request.id.as_str());
        let who = actor(&origin);
        leftover(
            self.store,
            &fixed,
            &request.requester,
            record,
            keep,
            &who,
            now,
        )
        .await
    }
}

/// The requester withdraws their open request.
///
/// # Errors
/// [`OwnershipError`].
pub async fn withdraw(
    store: &dyn ReadStore,
    request_id: &str,
    member: &str,
    now: DateTime<Utc>,
) -> Result<OwnerRequest, OwnershipError> {
    let request = store
        .owner_request(request_id.to_owned())
        .await?
        .ok_or(OwnershipError::UnknownRequest)?;
    ownership::may_withdraw(&request, member, now)?;
    let by = format!("member:{member}");
    if !store
        .close_owner_request(request.id.clone(), OwnerRequestStatus::Withdrawn, by, now)
        .await?
    {
        return Err(OwnershipRefusal::Closed.into());
    }
    store
        .owner_request(request.id)
        .await?
        .ok_or(OwnershipError::UnknownRequest)
}
