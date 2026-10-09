//! Weekly-timing ownership actions shared by Discord and the portal (user
//! decision 2026-10-10): hand off, ask, accept, decline, withdraw and expire.
//! Rules are `domain::ownership`; every owner change is a recorded fixed
//! edit pinning the new owner, keyed by the action so a retry replays it.

use chrono::{DateTime, Utc};

use crate::api::state::ReadStore;
use crate::api::write::{WriteContext, Writer};
use crate::domain::history::{Expect, Origin};
use crate::domain::ownership::{self, OwnerRequest, OwnerRequestStatus, OwnershipRefusal};
use crate::domain::schedule::{FixedEdit, FixedEditChoices, FixedEditRequest, FixedRun};
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

/// Pin `owner` with a recorded edit; a replay of the same `origin` request
/// id is not an error.
async fn pin(
    writer: &dyn Writer,
    origin: Origin,
    fixed_id: &str,
    owner: &str,
    ctx: &WriteContext,
) -> Result<(), OwnershipError> {
    let request = FixedEditRequest {
        fixed_id: fixed_id.to_owned(),
        edit: FixedEdit {
            owner_id: Some(owner.to_owned()),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::UpdateAll,
    };
    match writer
        .edit_fixed(origin, Expect::default(), request, ctx)
        .await
    {
        Ok(()) | Err(SchedulerError::AlreadyApplied { .. }) => Ok(()),
        Err(error) => Err(OwnershipError::Scheduler(error)),
    }
}

/// Close every other open request on the timing: its owner just changed.
async fn supersede(
    store: &dyn ReadStore,
    fixed_id: &str,
    keep: Option<&str>,
    actor: &str,
    now: DateTime<Utc>,
) -> Result<Vec<OwnerRequest>, OwnershipError> {
    let mut closed = Vec::new();
    for request in store.open_owner_requests().await? {
        if request.fixed_run_id != fixed_id || Some(request.id.as_str()) == keep {
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
    /// `origin` carries the action's request id (a retry replays).
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
        let fixed = timing(store, fixed_id).await?;
        let replayed = fixed.owner_pinned && fixed.owner() == to;
        if !replayed {
            ownership::hand_off(&fixed, giver, staff, to)?;
        }
        let who = actor(&origin);
        pin(writer, origin, fixed_id, to, ctx).await?;
        let superseded = supersede(store, fixed_id, None, &who, now).await?;
        Ok(OwnerChange {
            fixed: timing(store, fixed_id).await?,
            superseded,
        })
    }

    /// The owner (or staff) accepts or declines a request. Accepting pins the
    /// requester (request id `owner-request:<id>`, so a retry after a lost
    /// close replays) and supersedes the timing's other open requests.
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
        // A retry after the edit landed but before the close: finish the close.
        let landed = accept
            && request.status == OwnerRequestStatus::Open
            && fixed.owner_pinned
            && fixed.owner() == request.requester;
        if !landed {
            ownership::may_decide(&fixed, &request, decider, staff, accept, now)?;
        }
        let who = actor(&origin);
        let status = if accept {
            pin(
                writer,
                origin.with_request_id(format!("owner-request:{}", request.id)),
                &fixed.id,
                &request.requester,
                ctx,
            )
            .await?;
            OwnerRequestStatus::Accepted
        } else {
            OwnerRequestStatus::Declined
        };
        if !store
            .close_owner_request(request.id.clone(), status, who.clone(), now)
            .await?
        {
            return Err(OwnershipRefusal::Closed.into());
        }
        let superseded = if accept {
            supersede(store, &fixed.id, Some(&request.id), &who, now).await?
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
