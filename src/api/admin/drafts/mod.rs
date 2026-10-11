//! Drafts: the signed-in admin's staged schedule changes, previewed against
//! the current schedule and merged as one History record. Only the author
//! sees or writes a draft (anyone else gets the generic 404). Every write
//! names the version it was planned from and honours `Idempotency-Key`
//! (scope `drafts`, the persisted replay); nothing is live until a merge,
//! which is allowed only on the current head and without conflicts. Draft
//! writes hint `inbox`, a merge also `schedule`, on the events stream (the
//! store's write hook).

mod body;
mod read;
mod refusal;
mod write;

use std::sync::Arc;

use axum::{
    Router,
    routing::{delete, get, post},
};

use crate::api::listeners::Site;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/drafts", get(read::list).post(write::create))
        .route(
            "/api/admin/drafts/{id}",
            get(read::detail)
                .patch(write::retitle)
                .delete(write::discard),
        )
        .route("/api/admin/drafts/{id}/ops", post(write::add_op))
        .route("/api/admin/drafts/{id}/ops/{op}", delete(write::remove_op))
        .route("/api/admin/drafts/{id}/preview", get(read::preview))
        .route("/api/admin/drafts/{id}/rebase", post(write::rebase))
        .route("/api/admin/drafts/{id}/merge", post(write::merge))
}
