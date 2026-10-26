//! Draft writes through the one scheduler writer. With an `Idempotency-Key`
//! a write records its (small) answer under scope `drafts`: the same key
//! and request replays it, another request under the key is
//! `422 idempotency_mismatch`. The version precondition makes a concurrent
//! retry safe; it is re-checked against the key after a refusal, so the
//! loser of such a race still gets the first answer.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    Json,
    extract::{Path as UrlPath, State},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
};

use super::{
    super::{
        context::{context, roster},
        replay::{Keyed, replayed},
        write::{Body, Refusal, idempotency_key, state, write_context},
    },
    body::{AddOpBody, CreateBody, MergeBody, RebaseBody, RetitleBody, settle, version_param},
    read::own,
    refusal::{Doing, draft as refused, version_required},
};
use crate::{
    api::{
        auth::AdminSession,
        dto::drafts::{UNTITLED, written},
        error::ApiError,
        listeners::Site,
        state::ApiState,
        write::{DraftCommand, DraftDone},
    },
    infrastructure::store::replays::ReplayScope,
};
use kanade_domain::{
    drafts::{DraftStatus, StoredDraft},
    scheduler::{DraftError, Scope},
};

type Reply = Result<Response, Refusal>;

/// One draft write: replay a recorded key, else write and record the answer.
async fn keyed(
    state: &ApiState,
    session: &AdminSession,
    headers: &HeaderMap,
    identity: String,
    command: DraftCommand,
    doing: Doing,
) -> Reply {
    let now = state.now();
    let keyed = idempotency_key(headers)?
        .map(|key| Keyed::new(ReplayScope::Drafts, session, key.to_owned(), &identity));
    if let Some(keyed) = &keyed
        && let Some(found) = keyed.recall(state.store.as_ref(), now).await?
    {
        return replayed(&found);
    }
    let (ctx, _) = write_context(state).await?;
    let created = matches!(command, DraftCommand::Create { .. });
    let (id, version) = match &command {
        DraftCommand::Merge { id, version } => (id.clone(), *version),
        _ => (String::new(), 0),
    };
    let done = state.writer.draft(session.actor.id(), command, &ctx).await;
    let body = match done {
        Ok(DraftDone::Written(draft)) => answer(&draft, None),
        Ok(DraftDone::Merged(merge)) => merged(&id, version, merge.seq),
        // An exact retry of a merge without a key: answer as the first did.
        Err(DraftError::AlreadyApplied { seq, .. }) => merged(&id, version, seq),
        Err(error) => {
            // A concurrent request under the same key may have answered.
            if let Some(keyed) = &keyed
                && let Some(found) = keyed.recall(state.store.as_ref(), now).await?
            {
                return replayed(&found);
            }
            return Err(refused(error, doing));
        }
    };
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let value = serde_json::to_value(&body).map_err(Refusal::unavailable)?;
    if let Some(keyed) = keyed {
        state
            .store
            .put_replay(keyed.answered(status, &value, now))
            .await
            .map_err(Refusal::unavailable)?;
    }
    Ok((status, Json(value)).into_response())
}

fn answer(draft: &StoredDraft, seq: Option<u64>) -> crate::api::dto::drafts::DraftWritten {
    let message = match draft.status {
        DraftStatus::Discarded => "Draft discarded; nothing was applied.",
        _ => "Draft saved; nothing is live until it is merged.",
    };
    written(draft, seq, message.to_owned())
}

fn merged(id: &str, version: u64, seq: u64) -> crate::api::dto::drafts::DraftWritten {
    crate::api::dto::drafts::DraftWritten {
        id: id.to_owned(),
        version,
        status: "merged",
        seq: Some(seq),
        message: format!("Draft merged as change #{seq}."),
    }
}

/// The identity a key records: the action and its normalized arguments.
fn identity(parts: &[&str]) -> String {
    parts.join("\u{1f}")
}

pub async fn create(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    Body(body): Body<CreateBody>,
) -> Reply {
    let state = state(&site)?;
    let title = match body.title.as_deref().map(str::trim) {
        None | Some("") => UNTITLED.to_owned(),
        Some(title) => title.to_owned(),
    };
    // The key is also the domain's create request id, so a retry can never
    // make a second draft.
    let request_id = idempotency_key(&headers)?.map(str::to_owned);
    keyed(
        state,
        &session,
        &headers,
        identity(&["drafts.create", &title]),
        DraftCommand::Create { title, request_id },
        Doing::Staging,
    )
    .await
}

pub async fn retitle(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<RetitleBody>,
) -> Reply {
    let state = state(&site)?;
    let version = body.version.ok_or_else(version_required)?;
    own(state, &session, &id).await?;
    let title = body.title.trim().to_owned();
    keyed(
        state,
        &session,
        &headers,
        identity(&["drafts.retitle", &id, &version.to_string(), &title]),
        DraftCommand::Retitle { id, version, title },
        Doing::Staging,
    )
    .await
}

pub async fn discard(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    uri: Uri,
) -> Reply {
    let state = state(&site)?;
    let version = version_param(&uri)?.ok_or_else(version_required)?;
    own(state, &session, &id).await?;
    keyed(
        state,
        &session,
        &headers,
        identity(&["drafts.discard", &id, &version.to_string()]),
        DraftCommand::Discard { id, version },
        Doing::Staging,
    )
    .await
}

pub async fn add_op(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<AddOpBody>,
) -> Reply {
    let state = state(&site)?;
    let version = body.version.ok_or_else(version_required)?;
    own(state, &session, &id).await?;
    let current = state
        .store
        .snapshot(Scope::All)
        .await
        .map_err(Refusal::unavailable)?;
    let ctx = context(&site, state, roster(&[]), state.now());
    let op = body.op.op(&state.policy, &ctx, &current)?;
    let normalized = serde_json::to_string(&body.op).map_err(Refusal::unavailable)?;
    keyed(
        state,
        &session,
        &headers,
        identity(&["drafts.op", &id, &version.to_string(), &normalized]),
        DraftCommand::AddOp { id, version, op },
        Doing::Staging,
    )
    .await
}

pub async fn remove_op(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath((id, op)): UrlPath<(String, String)>,
    uri: Uri,
) -> Reply {
    let state = state(&site)?;
    let version = version_param(&uri)?.ok_or_else(version_required)?;
    let ord: usize = op
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| op.parse().ok())
        .flatten()
        .ok_or_else(|| Refusal::from(ApiError::NOT_FOUND))?;
    own(state, &session, &id).await?;
    keyed(
        state,
        &session,
        &headers,
        identity(&["drafts.remove", &id, &version.to_string(), &op]),
        DraftCommand::RemoveOp { id, version, ord },
        Doing::Staging,
    )
    .await
}

pub async fn rebase(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<RebaseBody>,
) -> Reply {
    let state = state(&site)?;
    let version = body.version.ok_or_else(version_required)?;
    let settle: BTreeMap<_, _> = settle(&body.settle)?;
    own(state, &session, &id).await?;
    let answers: Vec<String> = settle
        .iter()
        .map(|(key, answer)| format!("{key}={}", answer.as_str()))
        .collect();
    keyed(
        state,
        &session,
        &headers,
        identity(&[
            "drafts.rebase",
            &id,
            &version.to_string(),
            &answers.join(","),
        ]),
        DraftCommand::Rebase {
            id,
            version,
            settle,
        },
        Doing::Landing,
    )
    .await
}

pub async fn merge(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    Body(body): Body<MergeBody>,
) -> Reply {
    let state = state(&site)?;
    let version = body.version.ok_or_else(version_required)?;
    own(state, &session, &id).await?;
    keyed(
        state,
        &session,
        &headers,
        identity(&["drafts.merge", &id, &version.to_string()]),
        DraftCommand::Merge { id, version },
        Doing::Landing,
    )
    .await
}
