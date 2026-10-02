//! The Inbox (A6): extractor/chat proposals and member requests, listed with
//! the domain's merge previews, approved or rejected per source. Merge and
//! requester notices are written to the notice outbox by the store with the
//! decision; live serve attaches the shared proposal-card refresh after the
//! Discord desk is composed.

mod decide;
mod list;
mod messages;
mod refusal;

use std::sync::Arc;

use axum::{
    Router,
    routing::{get, post},
};

use crate::api::listeners::Site;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/inbox", get(list::list))
        .route("/api/admin/inbox/{id}/approve", post(decide::approve))
        .route("/api/admin/inbox/{id}/reject", post(decide::reject))
}
