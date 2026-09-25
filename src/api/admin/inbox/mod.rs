//! The Inbox (A6): extractor/chat proposals and member requests, listed with
//! the domain's merge previews, approved or rejected per source. Discord side
//! effects (card refresh, merge and requester notices) are returned by the
//! domain and dropped until serve composition, as A4 drops its notices.

mod decide;
mod list;
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
