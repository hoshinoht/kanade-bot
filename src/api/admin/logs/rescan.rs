//! Rescan targets and jobs. `POST` only queues a job through the rescan port
//! (the runner reads in its own task) and answers with it; `GET` polls it;
//! `DELETE` cancels it and is safe to repeat. `POST` and `DELETE` honour
//! `Idempotency-Key`: a replay answers the recorded job's current state, the
//! same key with another request is `422 idempotency_mismatch`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path as UrlPath, State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;

use super::{Reply, state};
use crate::api::admin::write::Refusal;
use crate::{
    api::{
        auth::AdminSession,
        dto::{Named, rescan::job},
        error::ApiError,
        listeners::Site,
        rescan::{Remembered, RescanDesk, RescanView, recall, remember},
        state::ApiState,
    },
    domain::history::Surface,
    extract::rescan::{API_WINDOWS, RescanError, RescanRequest},
};

use super::super::write::{bad_body, origin};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RescanBody {
    channels: Vec<String>,
    window: String,
}

fn desk(state: &ApiState) -> Result<&RescanDesk, Refusal> {
    state
        .rescans
        .as_deref()
        .ok_or_else(|| ApiError::UNAVAILABLE.into())
}

fn refused(error: RescanError) -> Refusal {
    match error {
        RescanError::NoChannels => Refusal::invalid("Choose at least one watched channel."),
        RescanError::Window(_) => {
            Refusal::invalid("Pick a window: this boss week, since reset or two weeks.")
        }
        RescanError::Off => Refusal::new(
            StatusCode::CONFLICT,
            "extraction_off",
            "Re-reading needs watching and the extractor switched on (Config → Watching).",
        ),
        RescanError::Closed | RescanError::Store(_) => ApiError::UNAVAILABLE.into(),
    }
}

fn mismatch() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "idempotency_mismatch",
        "That Idempotency-Key was already used for a different request.",
    )
}

fn channel_name(state: &ApiState) -> impl Fn(&str) -> String {
    let channels = state.channels.channels();
    move |id| {
        channels
            .iter()
            .find(|channel| channel.id == id)
            .map_or_else(|| id.to_owned(), |channel| channel.name.clone())
    }
}

fn answer(state: &ApiState, view: &RescanView) -> Reply {
    Ok(Json(job(view, channel_name(state))).into_response())
}

fn actor(session: &AdminSession) -> String {
    format!("{}:{}", session.actor.kind(), session.actor.id())
}

/// The recorded job's current state for a replayed key.
async fn replay(state: &ApiState, desk: &RescanDesk, job_id: &str) -> Reply {
    match desk.runner.job(job_id.to_owned()).await.map_err(refused)? {
        Some(view) => answer(state, &view),
        None => Err(ApiError::NOT_FOUND.into()),
    }
}

pub async fn targets(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let watched: Vec<Named> = state
        .channels
        .channels()
        .into_iter()
        .filter(|channel| channel.watched)
        .map(|channel| Named {
            id: channel.id,
            name: channel.name,
        })
        .collect();
    Ok(Json(watched).into_response())
}

pub async fn submit(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<RescanBody>, JsonRejection>,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let Json(body) = body.map_err(bad_body)?;
    let desk = desk(state)?;
    let mut channels: Vec<String> = Vec::new();
    for channel in body.channels {
        if !channels.contains(&channel) {
            channels.push(channel);
        }
    }
    let mut sorted = channels.clone();
    sorted.sort();
    let digest = format!("rescan\u{1f}{}\u{1f}{}", body.window, sorted.join("\u{1f}"));
    let actor = actor(&session);
    // Held to the end, so a concurrent retry with this key replays this job.
    let mut keys = match &key {
        Some(key) => {
            let keys = desk.keys().await;
            if let Some(entry) = recall(&keys, &actor, key) {
                if entry.digest != digest {
                    return Err(mismatch());
                }
                let job_id = entry.job_id.clone();
                drop(keys);
                return replay(state, desk, &job_id).await;
            }
            Some(keys)
        }
        None => None,
    };

    if !API_WINDOWS.contains(&body.window.as_str()) {
        return Err(refused(RescanError::Window(
            crate::extract::window::WindowError::Unknown {
                window: body.window,
            },
        )));
    }
    if channels.is_empty() {
        return Err(refused(RescanError::NoChannels));
    }
    let listed = state.channels.channels();
    let unwatched: Vec<&String> = channels
        .iter()
        .filter(|id| {
            !listed
                .iter()
                .any(|channel| &channel.id == *id && channel.watched)
        })
        .collect();
    match unwatched.as_slice() {
        [] => {}
        [one] => {
            let name = channel_name(state)(one);
            return Err(Refusal::invalid(format!(
                "{name} is not watched, so there is nothing to re-read."
            )));
        }
        _ => return Err(Refusal::invalid("Choose only watched channels.")),
    }

    let source = match session.origin().surface {
        Surface::Cli => "cli",
        _ => "portal",
    };
    let view = desk
        .runner
        .submit(RescanRequest {
            channels,
            window: body.window,
            source: source.to_owned(),
            automated: false,
            requested_by: Some(actor.clone()),
            unprocessed_only: false,
        })
        .await
        .map_err(refused)?;
    if let (Some(keys), Some(key)) = (keys.as_mut(), key) {
        remember(
            keys,
            Remembered {
                actor,
                key,
                digest,
                job_id: view.job.id.clone(),
            },
        );
    }
    answer(state, &view)
}

pub async fn poll(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    replay(state, desk, &id).await
}

pub async fn cancel(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let desk = desk(state)?;
    let digest = format!("cancel\u{1f}{id}");
    let actor = actor(&session);
    let mut keys = match &key {
        Some(key) => {
            let keys = desk.keys().await;
            if let Some(entry) = recall(&keys, &actor, key) {
                if entry.digest != digest {
                    return Err(mismatch());
                }
                drop(keys);
                return replay(state, desk, &id).await;
            }
            Some(keys)
        }
        None => None,
    };
    let view = desk
        .runner
        .cancel(id.clone())
        .await
        .map_err(refused)?
        .ok_or(ApiError::NOT_FOUND)?;
    if let (Some(keys), Some(key)) = (keys.as_mut(), key) {
        remember(
            keys,
            Remembered {
                actor,
                key,
                digest,
                job_id: id,
            },
        );
    }
    answer(state, &view)
}
