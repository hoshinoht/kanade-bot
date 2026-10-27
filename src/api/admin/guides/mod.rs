//! The guide publisher (module `guides`, plan section 7): admins write
//! guides as blocks, preview them as Discord shows them and publish them to
//! one channel. While the module is off (`modules.guides`, read at startup;
//! serve attaches [`GuidePort`] only then) every route answers the generic
//! `404` before the session is checked, as an unmounted path does. Without
//! the bot (the `web` component alone) the port is
//! [`crate::app::ports::OfflineGuides`]: guides stay editable and every run
//! refuses `discord_offline`. Writes
//! take the session's CSRF token and replay an `Idempotency-Key` (scope
//! `config`, identity = the full normalized request) in the same
//! transaction; History rows (section `guides`) are the store's, written
//! only with an effect. The API never holds a transport: plans read the
//! channel through [`GuidePort::inspect`] and runs are queued in the store
//! for the worker, which checks the channel again before acting.

mod read;
mod status;
mod write;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Serialize;

use super::write::Refusal;
use crate::{
    api::{
        dto::guides::{GuideBlocksRefused, problems},
        error::ApiError,
        listeners::Site,
        state::ApiState,
    },
    app::{ports::GuidePort, read::GuideBook},
};
use kanade_domain::guides::{Block, BlockError, Guide, Rendered, render};
use kanade_files::GuideFiles;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/guides", get(read::list).post(write::create))
        .route(
            "/api/admin/guides/{id}",
            get(read::detail).patch(write::save).delete(write::delete),
        )
        .route("/api/admin/guides/{id}/render", post(read::preview))
        .route("/api/admin/guides/{id}/plan", get(read::plan))
        .route("/api/admin/guides/{id}/publish", post(write::publish))
        .route("/api/admin/guides/{id}/unpublish", post(write::unpublish))
        .route("/api/admin/guides/{id}/search", post(write::search))
        .route(
            "/api/admin/guides/{id}/slots/not-posted",
            post(write::not_posted),
        )
        .route("/api/admin/guides/{id}/publishes/{run}", get(read::run))
        .route(
            "/api/admin/guides/{id}/publishes/{run}/cancel",
            post(write::cancel),
        )
}

type Reply = Result<Response, Refusal>;

/// The module's state and port; the generic `404` while the module is off.
/// Taken first, so an off module answers as an unmounted path whoever asks.
struct Guides {
    state: Arc<ApiState>,
    port: Arc<dyn GuidePort>,
}

impl FromRequestParts<Arc<Site>> for Guides {
    type Rejection = ApiError;

    async fn from_request_parts(_: &mut Parts, site: &Arc<Site>) -> Result<Self, ApiError> {
        let state = site.state.as_ref().ok_or(ApiError::NOT_FOUND)?;
        let port = state.guides.clone().ok_or(ApiError::NOT_FOUND)?;
        Ok(Self {
            state: Arc::clone(state),
            port,
        })
    }
}

impl Guides {
    fn book(&self) -> &dyn GuideBook {
        self.state.store.guide_book()
    }

    async fn guide(&self, id: &str) -> Result<Guide, Refusal> {
        self.book()
            .guide(id.to_owned())
            .await
            .map_err(Refusal::unavailable)?
            .ok_or_else(not_found)
    }

    /// A worker claims runs and the guild is loaded; otherwise publish,
    /// unpublish and "Search again" refuse `discord_offline`.
    fn can_publish(&self) -> bool {
        self.port.publishing() && self.state.channels.connected()
    }
}

fn not_found() -> Refusal {
    Refusal::not_found("not_found", "That guide is gone.")
}

fn stale() -> Refusal {
    Refusal::conflict(
        "stale",
        "Someone saved a newer version of this guide since it was loaded.",
    )
}

fn publishing() -> Refusal {
    Refusal::conflict(
        "publishing",
        "This guide is publishing; wait for it to finish or cancel it first.",
    )
}

fn channel_taken() -> Refusal {
    Refusal::conflict("channel_taken", "Another guide already uses that channel.")
}

/// A JSON answer other than an `ApiError` (`429`-style extra fields).
fn answer(status: StatusCode, body: &impl Serialize) -> Response {
    (status, Json(body)).into_response()
}

/// `422`: the blocks (as sent, or the saved draft) do not render.
fn blocks_refused(error: &'static str, errors: &[BlockError]) -> Response {
    let message = match errors {
        [only] => only.to_string(),
        _ => format!("{} problems in the blocks; fix them first.", errors.len()),
    };
    answer(
        StatusCode::UNPROCESSABLE_ENTITY,
        &GuideBlocksRefused {
            error,
            message,
            problems: problems(errors),
        },
    )
}

/// What rendering reads: the catalog, the boss directory and knowledge.
#[derive(Clone)]
struct Renderer {
    catalog: Arc<kanade_domain::catalog::BossTable>,
    files: GuideFiles,
}

impl Renderer {
    fn new(site: &Site, state: &ApiState) -> Self {
        Self {
            catalog: Arc::clone(&state.catalog),
            files: GuideFiles::new(site.boss_dir.as_deref(), state.knowledge_dir.as_deref()),
        }
    }

    /// Render each draft off the async workers (art reads block), as the
    /// publish worker renders: the same sources give the same hashes.
    async fn render_all(
        &self,
        drafts: Vec<Vec<Block>>,
    ) -> Result<Vec<Result<Rendered, Vec<BlockError>>>, Refusal> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            drafts
                .iter()
                .map(|blocks| render(blocks, &this.catalog, &this.files))
                .collect()
        })
        .await
        .map_err(Refusal::unavailable)
    }

    async fn render(
        &self,
        blocks: Vec<Block>,
    ) -> Result<Result<Rendered, Vec<BlockError>>, Refusal> {
        let mut all = self.render_all(vec![blocks]).await?;
        all.pop().ok_or_else(|| ApiError::UNAVAILABLE.into())
    }
}
