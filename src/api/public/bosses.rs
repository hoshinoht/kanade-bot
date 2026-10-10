//! The signed-in member's Bosses page (catalog C7/C8): the catalog list and
//! event bosses as the admin reads build them (neither carries an
//! operator-only field), and knowledge pages as [`PublicKnowledge`], without
//! the repository path and the chatbot-only `detail` wording.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::PathRejection},
};

use crate::api::{
    admin::read::{boss_knowledge, boss_rows, event_bosses},
    auth::member::MemberSession,
    dto::bosses::{BossRow, EventBoss, PublicKnowledge},
    error::ApiError,
    listeners::Site,
};

/// `GET /api/public/bosses`: every catalog boss in catalog order.
pub(super) async fn list(
    State(site): State<Arc<Site>>,
    _: MemberSession,
) -> Result<Json<Vec<BossRow>>, ApiError> {
    boss_rows(&site).await.map(Json)
}

/// `GET /api/public/bosses/events`: knowledge documents that declare an event.
pub(super) async fn events(
    State(site): State<Arc<Site>>,
    _: MemberSession,
) -> Result<Json<Vec<EventBoss>>, ApiError> {
    event_bosses(&site).await.map(Json)
}

/// `GET /api/public/bosses/{key}/knowledge`.
pub(super) async fn knowledge(
    State(site): State<Arc<Site>>,
    _: MemberSession,
    key: Result<UrlPath<String>, PathRejection>,
) -> Result<Json<PublicKnowledge>, ApiError> {
    let UrlPath(key) = key.map_err(|_| ApiError::NOT_FOUND)?;
    let knowledge = boss_knowledge(&site, &key).await?;
    Ok(Json(knowledge.into()))
}
