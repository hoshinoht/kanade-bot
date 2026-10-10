//! The member's own requests: the list with their limits and the form's
//! choices, submit and withdraw. Requests are admin-reviewed drafts (the
//! Inbox decides them); they are written through the one writer as the
//! member. Someone else's request and an unknown one are the same 404.

mod form;

use std::{collections::BTreeSet, sync::Arc};

use axum::{
    Json,
    body::Bytes,
    extract::{
        Path as UrlPath, State,
        rejection::{JsonRejection, PathRejection},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::write::{admit, invalid_body, keyed_id, path_id};
use crate::{
    api::{
        admin::{
            context::{context, frames, roster, run_lengths, unavailable},
            write::{Refusal, scheduler, state, write_context_of},
        },
        auth::{audit::AuditContext, member::MemberSession},
        dto::{
            Named, iso_instant,
            public::{
                MemberRequest, MemberRequestLimit, MemberRequestOptions, MemberRequests,
                RequestView,
            },
            week::{Context, WeekFrame},
        },
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    domain::{
        drafts::{DraftKind, DraftStatus, LoadedDraft, RequestLimit},
        history::Actor,
        members::MemberProfile,
        requests::{DEFAULT_LIMITS, RequestRefusal, Subject, public_summary},
        schedule::ScheduleSnapshot,
        scheduler::{DraftError, RequestError, Scope},
        settings::RunLengths,
    },
};
use form::{Choices, RequestBody};

/// A refusal; `request_limit` also names the limit that refused.
pub(super) enum Refused {
    Plain(Refusal),
    Limit(MemberRequestLimit),
}

impl From<Refusal> for Refused {
    fn from(refusal: Refusal) -> Self {
        Self::Plain(refusal)
    }
}

impl From<ApiError> for Refused {
    fn from(error: ApiError) -> Self {
        Self::Plain(error.into())
    }
}

impl IntoResponse for Refused {
    fn into_response(self) -> Response {
        match self {
            Self::Plain(refusal) => refusal.into_response(),
            Self::Limit(body) => (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response(),
        }
    }
}

fn not_found() -> Refusal {
    Refusal::new(
        StatusCode::NOT_FOUND,
        "not_found",
        "There is no such request, run or weekly timing for you.",
    )
}

fn closed() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "request_closed",
        "That request is no longer waiting for a decision.",
    )
}

fn no_effect() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "no_effect",
        "That request could not apply to the schedule as it is now.",
    )
}

/// Status and code per domain refusal.
fn refusal(error: RequestError) -> Refused {
    let limit = |limit: &'static str, message: String| {
        Refused::Limit(MemberRequestLimit {
            error: "request_limit",
            message,
            limit,
        })
    };
    let plain = match error {
        RequestError::Refused(refused) => match refused {
            RequestRefusal::RequesterUnauthorised | RequestRefusal::UnknownSubject => not_found(),
            RequestRefusal::AlreadyInParty => Refusal::new(
                StatusCode::CONFLICT,
                "already_in_party",
                refused.to_string(),
            ),
            RequestRefusal::FieldNotAllowed => Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "field_not_allowed",
                refused.to_string(),
            ),
            RequestRefusal::Frozen => {
                Refusal::new(StatusCode::FORBIDDEN, "forbidden", refused.to_string())
            }
            _ => invalid_body(),
        },
        RequestError::Limited(RequestLimit::Pending { max, .. }) => {
            return limit(
                "open",
                format!("At most {max} of your requests can wait for a decision at once."),
            );
        }
        RequestError::Limited(RequestLimit::Rate { max, .. }) => {
            return limit(
                "today",
                format!("At most {max} requests a day; try again later."),
            );
        }
        RequestError::Draft(error) => match error {
            DraftError::RequestMismatch { .. } => Refusal::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "idempotency_mismatch",
                "That Idempotency-Key was already used for a different request.",
            ),
            DraftError::InvalidTitle => invalid_body(),
            DraftError::UnknownDraft(_) | DraftError::RequesterUnauthorised => not_found(),
            DraftError::Stale { .. } | DraftError::AlreadyMerged { .. } => closed(),
            DraftError::ReplayFailed { .. }
            | DraftError::Conflicts(_)
            | DraftError::Expired
            | DraftError::NoEffect
            | DraftError::Empty => no_effect(),
            DraftError::Store(error) => scheduler(error.into()),
            _ => ApiError::UNAVAILABLE.into(),
        },
        RequestError::AdminDraft => not_found(),
        RequestError::Expired(_) => closed(),
        RequestError::NoEffect => no_effect(),
    };
    Refused::Plain(plain)
}

/// What every request read starts from.
struct Basis {
    now: DateTime<Utc>,
    frames: [WeekFrame; 2],
    /// This and next boss week's runs, and every weekly timing.
    base: ScheduleSnapshot,
    profiles: Vec<MemberProfile>,
    run_lengths: RunLengths,
}

impl Basis {
    async fn load(state: &ApiState) -> Result<Self, Refusal> {
        let now = state.now();
        let frames = frames(state, now)?;
        let base = state
            .store
            .snapshot(Scope::Weeks(vec![frames[0].start, frames[1].start]))
            .await
            .map_err(unavailable)?;
        let profiles = state.store.members().await.map_err(unavailable)?;
        Ok(Self {
            now,
            frames,
            base,
            profiles,
            run_lengths: run_lengths(state).await,
        })
    }

    fn view<'a>(
        &'a self,
        state: &'a ApiState,
        ctx: &'a Context<'a>,
        user_id: &'a str,
    ) -> RequestView<'a> {
        RequestView {
            ctx,
            frames: &self.frames,
            policy: &state.policy,
            run_lengths: &self.run_lengths,
            user_id,
        }
    }

    /// The form's choices: watched channels this and next boss week's runs
    /// or the weekly timings use, and every member with the bossing role.
    fn options(&self, ctx: &Context<'_>) -> MemberRequestOptions {
        let used: BTreeSet<&str> = self
            .base
            .runs
            .iter()
            .filter_map(|run| run.channel_id.as_deref())
            .chain(
                self.base
                    .fixed_runs
                    .iter()
                    .filter_map(|fixed| fixed.channel_id.as_deref()),
            )
            .collect();
        let mut channels: Vec<Named> = used
            .into_iter()
            .filter(|id| ctx.channels.get(*id).is_some_and(|channel| channel.watched))
            .map(|id| Named {
                id: id.to_owned(),
                name: ctx.channel_name(id),
            })
            .collect();
        let mut members: Vec<Named> = self
            .profiles
            .iter()
            .filter(|profile| profile.member.has_role && !profile.member.is_bot)
            .map(|profile| ctx.named(&profile.member.user_id))
            .collect();
        for list in [&mut channels, &mut members] {
            list.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));
        }
        MemberRequestOptions { channels, members }
    }
}

/// The request's run subject when this and next boss week do not hold it.
fn outside(base: &ScheduleSnapshot, loaded: &LoadedDraft) -> Option<String> {
    match loaded.draft.subject.as_deref().and_then(Subject::parse) {
        Some(Subject::Run(id)) if !base.runs.iter().any(|run| run.id == id) => Some(id),
        _ => None,
    }
}

/// One request as the member sees it; a run subject outside this and next
/// boss week is read on its own.
async fn project(
    state: &ApiState,
    view: &RequestView<'_>,
    base: &ScheduleSnapshot,
    loaded: &LoadedDraft,
) -> Result<MemberRequest, Refusal> {
    if let Some(id) = outside(base, loaded) {
        let own = state
            .store
            .snapshot(Scope::Run(id))
            .await
            .map_err(unavailable)?;
        return Ok(view.request(loaded, &own));
    }
    Ok(view.request(loaded, base))
}

/// The member's own request `id`; anyone else's is the uniform 404.
async fn own_request(state: &ApiState, id: String, me: &str) -> Result<LoadedDraft, Refusal> {
    state
        .store
        .draft(id)
        .await
        .map_err(unavailable)?
        .filter(|loaded| {
            loaded.draft.kind == DraftKind::Request && loaded.draft.author == Actor::member(me)
        })
        .ok_or_else(not_found)
}

/// `GET /api/public/requests/mine`: newest first, with the limits and the
/// form's choices.
pub(super) async fn mine(
    State(site): State<Arc<Site>>,
    session: MemberSession,
) -> Result<Json<MemberRequests>, Refusal> {
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let basis = Basis::load(state).await?;
    let requests = state
        .store
        .member_requests(me.to_owned())
        .await
        .map_err(unavailable)?;
    let ctx = context(&site, state, roster(&basis.profiles), basis.now);
    let view = basis.view(state, &ctx, me);
    // As the store counts them for the limits.
    let since = basis.now - DEFAULT_LIMITS.window;
    let open = requests
        .iter()
        .filter(|loaded| loaded.draft.status == DraftStatus::Submitted)
        .count();
    let today = requests
        .iter()
        .filter(|loaded| loaded.draft.created_at > since)
        .count();
    // Waiting requests about older runs (shown expired) read those runs'
    // weeks in one go; a closed one's older run is shown as gone.
    let weeks: BTreeSet<DateTime<Utc>> = requests
        .iter()
        .filter(|loaded| loaded.draft.status.is_live() && outside(&basis.base, loaded).is_some())
        .filter_map(|loaded| loaded.draft.scope.expires_week())
        .collect();
    let old = if weeks.is_empty() {
        ScheduleSnapshot::default()
    } else {
        state
            .store
            .snapshot(Scope::Weeks(weeks.into_iter().collect()))
            .await
            .map_err(unavailable)?
    };
    let shown = requests
        .iter()
        .map(|loaded| {
            let on = if loaded.draft.status.is_live() && outside(&basis.base, loaded).is_some() {
                &old
            } else {
                &basis.base
            };
            view.request(loaded, on)
        })
        .collect();
    let count = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
    Ok(Json(MemberRequests {
        requests: shown,
        open: count(open as u64),
        today: count(today as u64),
        max_open: count(DEFAULT_LIMITS.max_pending),
        max_today: count(DEFAULT_LIMITS.max_per_window),
        options: basis.options(&ctx),
        generated_at: iso_instant(basis.now),
    }))
}

/// `POST /api/public/requests`: `201` with the new request; a retry with the
/// same key answers `200` with that request as it is now.
pub(super) async fn submit(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    body: Result<Json<RequestBody>, JsonRejection>,
) -> Result<(StatusCode, Json<MemberRequest>), Refused> {
    let (member, key) = admit(&site, &audit, &session, &headers)?;
    session.require_fresh(member.now())?;
    let Ok(Json(body)) = body else {
        return Err(invalid_body().into());
    };
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let request_id = keyed_id(me, key);
    // A retry answers (or is refused as a mismatch) before any live check.
    let retry = state
        .store
        .recorded_request(me.to_owned(), request_id.clone())
        .await
        .map_err(unavailable)?
        .is_some();
    let basis = Basis::load(state).await?;
    let ctx = context(&site, state, roster(&basis.profiles), basis.now);
    let options = basis.options(&ctx);
    let choices = Choices {
        channels: options.channels.iter().map(|c| c.id.as_str()).collect(),
        members: options.members.iter().map(|m| m.id.as_str()).collect(),
        catalog: &state.catalog,
        lenient: retry,
    };
    let checked = form::check(body, me, &choices)?;
    let subject = checked.spec.subject();
    // Members ask about this and next boss week's runs only.
    if !retry
        && let Some(Subject::Run(id)) = &subject
        && !basis.base.runs.iter().any(|run| &run.id == id)
    {
        return Err(not_found().into());
    }
    let title = checked
        .note
        .unwrap_or_else(|| public_summary(checked.spec.request_type(), subject.as_ref()));
    let write = write_context_of(state, &basis.profiles);
    let (draft, created) = state
        .writer
        .submit_request(me, &title, checked.spec, request_id, &write)
        .await
        .map_err(refusal)?;
    let loaded = own_request(state, draft.id, me).await?;
    let view = basis.view(state, &ctx, me);
    let request = project(state, &view, &basis.base, &loaded).await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(request)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

/// `POST /api/public/requests/{id}/withdraw` (`{}` or no body): the
/// requester, while it waits. Withdrawing it again answers it as it is.
pub(super) async fn withdraw(
    State(site): State<Arc<Site>>,
    audit: AuditContext,
    session: MemberSession,
    headers: HeaderMap,
    path: Result<UrlPath<String>, PathRejection>,
    body: Bytes,
) -> Result<Json<MemberRequest>, Refused> {
    admit(&site, &audit, &session, &headers)?;
    let id = path_id(path)?;
    if !body.is_empty() && serde_json::from_slice::<Empty>(&body).is_err() {
        return Err(invalid_body().into());
    }
    let state = state(&site)?;
    let me = session.user_id.as_str();
    let loaded = own_request(state, id.clone(), me).await?;
    let basis = Basis::load(state).await?;
    let ctx = context(&site, state, roster(&basis.profiles), basis.now);
    let view = basis.view(state, &ctx, me);
    let retried = loaded.draft.status == DraftStatus::Withdrawn
        && loaded.draft.closed_by == Some(Actor::member(me));
    let loaded = if retried {
        loaded
    } else {
        if !view.waiting(&loaded.draft) {
            return Err(closed().into());
        }
        state
            .writer
            .withdraw_request(me, &id, loaded.draft.version)
            .await
            .map_err(refusal)?;
        own_request(state, id, me).await?
    };
    Ok(Json(project(state, &view, &basis.base, &loaded).await?))
}
