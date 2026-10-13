//! The member's own characters on the public origin: their roster rows
//! (add, edit, retire; never delete) with module `roster`, and with module
//! `character_data` the NEXON lookup preview, the link notice (v1, word for
//! word as Discord's), Link / Cancel / Unlink, My data and their own
//! scouter lookups; and, on a run they are on, the party's characters.
//!
//! Every route answers the generic `404` while its module is off, before
//! the session is checked (`roster`; `character_data` for the NEXON and
//! estimate routes; the scouter URL for the estimates). A character that
//! is not the caller's answers the same generic `404` as an unknown id, and
//! another member's lookup job is `404` too. Roster writes and Link take a
//! member write token, an `Idempotency-Key` and a fresh sign-in; Unlink
//! takes the token and key only; the lookup preview has its own bucket (10
//! per 10 minutes) and a lookup start one per 30 minutes (failures do not
//! count). History rows name the member, surface `public_portal`.

mod estimates;
mod link;
mod party;
mod roster;

use std::sync::Arc;

use axum::{
    Router,
    extract::{Request, State},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};

use super::{closed, read_bucket, unmounted, write::audit_refusals};
use crate::{
    api::{
        admin::write::Refusal,
        auth::member,
        error::ApiError,
        listeners::Site,
        state::{ApiState, CharacterRoster},
    },
    domain::characters::Character,
    infrastructure::character_data::CharacterData,
};

/// Each path is mounted once; its reads take the member's read token, and
/// a refused write is audited (layers added after a method wrap the
/// methods before them too; the audit passes reads through).
pub(super) fn routes(site: &Arc<Site>) -> Router<Arc<Site>> {
    let read = || from_fn_with_state(site.clone(), read_bucket);
    let audited = || from_fn_with_state(site.clone(), audit_refusals);
    let roster = Router::new()
        .route(
            "/api/public/me/characters",
            get(roster::list)
                .route_layer(read())
                .post(roster::create)
                .route_layer(audited())
                .fallback(unmounted),
        )
        .route(
            "/api/public/me/characters/{id}",
            patch(roster::update)
                .route_layer(audited())
                .fallback(unmounted),
        )
        .route(
            "/api/public/runs/{id}/characters",
            get(party::read).route_layer(read()).fallback(unmounted),
        );
    let nexon = Router::new()
        .route(
            "/api/public/me/characters/lookup",
            post(link::look_up).route_layer(read()).fallback(unmounted),
        )
        .route(
            "/api/public/me/characters/{id}/link",
            get(link::status)
                .route_layer(read())
                .post(link::confirm)
                .delete(link::unlink)
                .route_layer(audited())
                .fallback(unmounted),
        )
        .route(
            "/api/public/me/characters/{id}/link/notice",
            get(link::notice).route_layer(read()).fallback(unmounted),
        )
        .route(
            "/api/public/me/characters/{id}/link/cancel",
            post(link::cancel).route_layer(read()).fallback(unmounted),
        )
        .route(
            "/api/public/me/characters/links",
            delete(link::unlink_all)
                .route_layer(audited())
                .fallback(unmounted),
        )
        .route(
            "/api/public/me/data",
            get(link::data).route_layer(read()).fallback(unmounted),
        );
    let scouter = Router::new()
        .route(
            "/api/public/me/estimates",
            get(estimates::status)
                .post(estimates::start)
                .route_layer(read())
                .fallback(unmounted),
        )
        .route(
            "/api/public/me/estimates/{job}",
            get(estimates::read).route_layer(read()).fallback(unmounted),
        );
    Router::new()
        .merge(gated(site, roster, Need::Roster))
        .merge(gated(site, nexon, Need::CharacterData))
        .merge(gated(site, scouter, Need::Estimates))
}

#[derive(Clone, Copy)]
enum Need {
    Roster,
    CharacterData,
    Estimates,
}

impl Need {
    fn met(self, site: &Site) -> bool {
        let Some(state) = &site.state else {
            // No read state: the handlers answer `unavailable` as the
            // other member reads do.
            return true;
        };
        let module = state.character_data.as_ref();
        state.characters.is_some()
            && match self {
                Self::Roster => true,
                Self::CharacterData => module.is_some(),
                Self::Estimates => module.is_some_and(|module| module.estimates().is_some()),
            }
    }
}

/// The session check inside the module gate inside `closed`: a portal that
/// is closed answers `closed`, an off module the generic `404`, both
/// before the session is read.
fn gated(site: &Arc<Site>, routes: Router<Arc<Site>>, need: Need) -> Router<Arc<Site>> {
    routes
        .route_layer(from_fn_with_state(site.clone(), member::require_session))
        .route_layer(from_fn_with_state((site.clone(), need), gate))
        .route_layer(from_fn_with_state(site.clone(), closed))
}

async fn gate(
    State((site, need)): State<(Arc<Site>, Need)>,
    request: Request,
    next: Next,
) -> Response {
    if need.met(&site) {
        next.run(request).await
    } else {
        ApiError::NOT_FOUND.into_response()
    }
}

fn unavailable<E>(_: E) -> Refusal {
    ApiError::UNAVAILABLE.into()
}

/// The roster (its gate let the request through).
fn roster(state: &ApiState) -> Result<&Arc<dyn CharacterRoster>, Refusal> {
    state
        .characters
        .as_ref()
        .ok_or_else(|| ApiError::NOT_FOUND.into())
}

/// Module `character_data` (its gate let the request through).
fn module(state: &ApiState) -> Result<&Arc<CharacterData>, Refusal> {
    state
        .character_data
        .as_ref()
        .ok_or_else(|| ApiError::NOT_FOUND.into())
}

/// The caller's own roster character `id`; anyone else's is the same
/// generic `404` as an unknown id.
async fn own(roster: &dyn CharacterRoster, me: &str, id: &str) -> Result<Character, Refusal> {
    match roster.character(id.to_owned()).await {
        Ok(Some(character)) if character.member_id == me => Ok(character),
        Ok(_) => Err(ApiError::NOT_FOUND.into()),
        Err(error) => Err(unavailable(error)),
    }
}
