//! `GET /api/admin/history/sign-ins`: the stored sign-in audit log (user
//! decision 2026-10-09), newest first, filtered by `realm`, `event`, `actor`
//! and guild-local `from`/`to` dates; `before` pages older rows.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::State,
    http::Uri,
    response::{IntoResponse, Response},
};

use super::super::{
    context::{state, unavailable},
    logs::filter,
    write::Refusal,
};
use crate::api::{auth::AdminSession, dto::sign_ins, listeners::Site};

pub(super) async fn page(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    uri: Uri,
) -> Result<Response, Refusal> {
    let state = state(&site)?;
    let filter = filter::sign_ins(&uri, state.policy.zone())?;
    let rows = state
        .store
        .audit_page(filter.clone())
        .await
        .map_err(unavailable)?;
    let names: BTreeMap<String, String> = state
        .store
        .members()
        .await
        .map_err(unavailable)?
        .into_iter()
        .filter_map(|profile| {
            let member = profile.member;
            let name = member.nickname.or(member.display_name)?;
            Some((member.user_id, name))
        })
        .collect();
    let full = rows.len() == filter.page_size() as usize;
    Ok(Json(sign_ins::SignInPage {
        next_before: full.then(|| rows.last().map(|row| row.seq)).flatten(),
        rows: rows
            .iter()
            .map(|row| sign_ins::row(row, |id| names.get(id).cloned()))
            .collect(),
    })
    .into_response())
}
