//! Runtime settings (A9): `GET`/`PATCH /api/admin/config` and the profile
//! reload. One lock serialises saves; each saved change is published on
//! [`ConfigDesk::subscribe`]. `PATCH` honours `Idempotency-Key` (a replay
//! answers the current view with the first request's notices; the same key
//! for another body is `422 idempotency_mismatch`); the reload is naturally
//! repeatable.

mod desk;
mod models;
mod patch;
mod stack;

use std::{convert::Infallible, sync::Arc};

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};

pub use desk::{
    CatalogRead, ConfigDesk, ConfigFacts, ConfigFuture, ConfigInputs, ModelCatalog, PersonaFiles,
    SettingsChanged, SettingsPort,
};

use super::write::{Refusal, bad_body, origin, state};
use crate::{
    api::{auth::AdminSession, error::ApiError, listeners::Site, state::ApiState},
    chat::persona::{FALLBACK_PERSONA, PersonaId, PersonaRoot, ReloadError},
    domain::settings::{Persona, RuntimeSettings, Section, SettingsError},
};

type Reply = Result<axum::response::Response, Refusal>;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/config", get(read).patch(update))
        .route("/api/admin/config/profiles/reload", post(reload_profiles))
}

fn desk(state: &ApiState) -> Result<&ConfigDesk, Refusal> {
    state
        .config
        .as_deref()
        .ok_or_else(|| ApiError::UNAVAILABLE.into())
}

fn mismatch() -> Refusal {
    Refusal::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "idempotency_mismatch",
        "That Idempotency-Key was already used for a different request.",
    )
}

fn models_unreachable(message: &str) -> Refusal {
    Refusal::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "models_unreachable",
        message,
    )
}

fn stored(error: SettingsError) -> Refusal {
    match error {
        SettingsError::Store(_) => ApiError::UNAVAILABLE.into(),
        SettingsError::Malformed { .. } | SettingsError::Unrepresentable { .. } => {
            Refusal::invalid("That value cannot be stored.")
        }
    }
}

fn put(settings: &mut RuntimeSettings, section: Section) {
    match section {
        Section::Pings(value) => settings.pings = value,
        Section::Watching(value) => settings.watching = value,
        Section::Chatbot(value) => settings.chatbot = value,
        Section::Notifications(value) => settings.notifications = value,
        Section::SelfService(value) => settings.self_service = value,
        Section::Persona(value) => settings.persona = value,
        Section::Models(value) => settings.models = value,
        Section::Schedule(value) => settings.schedule = value,
        Section::Posting(value) => settings.posting = value,
    }
}

fn answer(
    state: &ApiState,
    desk: &ConfigDesk,
    settings: &RuntimeSettings,
    catalog: &CatalogRead,
    notices: Vec<String>,
) -> Reply {
    let view = desk.view(settings, catalog, &state.channels.channels(), notices);
    Ok(Json(view).into_response())
}

async fn read(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    let settings = desk.settings().await;
    let catalog = desk.catalog().await;
    answer(state, desk, &settings, &catalog, Vec::new())
}

async fn update(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Reply {
    let state = state(&site)?;
    let key = origin(&session, &headers)?.request_id;
    let Json(body) = body.map_err(bad_body)?;
    let desk = desk(state)?;
    let actor = format!("{}:{}", session.actor.kind(), session.actor.id());
    // serde_json maps are sorted, so key order never changes the digest.
    let digest = body.to_string();

    let mut current = desk.lock().await;
    if let Some(key) = &key
        && let Some(entry) = desk.recall(&actor, key)
    {
        if entry.digest != digest {
            return Err(mismatch());
        }
        let settings = current.clone();
        drop(current);
        let catalog = desk.catalog().await;
        return answer(state, desk, &settings, &catalog, entry.notices);
    }

    let (name, fields) = patch::section(&body)?;
    let mut notices = Vec::new();
    let mut catalog = None;
    let section = match name {
        "pings" => Section::Pings(patch::pings(&current.pings, fields)?),
        "watching" => Section::Watching(patch::watching(&current.watching, fields)?),
        "chatbot" => Section::Chatbot(patch::chatbot(
            &current.chatbot,
            fields,
            &desk.missing_env(&current),
        )?),
        "notifications" => Section::Notifications(patch::notifications(fields)?),
        "self_service" => Section::SelfService(patch::self_service(&current.self_service, fields)?),
        "persona" => Section::Persona(Persona {
            active: patch::persona_active(fields)?,
        }),
        _ => {
            if desk.models.is_none() {
                return Err(models_unreachable(
                    "No model gateway is configured, so model settings cannot be checked.",
                ));
            }
            let read = desk.catalog().await;
            if !read.reachable {
                return Err(models_unreachable(
                    "Kanata is unreachable, so model settings cannot be checked; try again shortly.",
                ));
            }
            let (next, reset) =
                models::apply_roles(&current.models, &fields["roles"], &read.snapshot)?;
            let declared = !desk.facts.model_groups.is_empty();
            let before = models::capacity(
                &current.models,
                &desk.groups(&current),
                declared,
                Some(&read.snapshot),
            );
            let mut proposed = current.clone();
            proposed.models = next.clone();
            let after = models::capacity(
                &next,
                &desk.groups(&proposed),
                declared,
                Some(&read.snapshot),
            );
            let errors = models::new_errors(&before, &after);
            if !errors.is_empty() {
                return Err(Refusal::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "capacity",
                    errors.join(" "),
                ));
            }
            notices = reset;
            catalog = Some(read);
            Section::Models(next)
        }
    };

    let mut next = current.clone();
    put(&mut next, section.clone());
    if let Section::Persona(persona) = &section {
        switch_persona(desk, &persona.active, section.clone()).await?;
    } else if next != *current {
        desk.store.save(section).await.map_err(stored)?;
    }
    if next != *current {
        *current = next.clone();
        desk.publish(name_of(name), actor.clone(), &next);
    }
    drop(current);
    if let Some(key) = key {
        desk.remember(desk::Remembered {
            actor,
            key,
            digest,
            notices: notices.clone(),
        });
    }
    let catalog = match catalog {
        Some(catalog) => catalog,
        None => desk.catalog().await,
    };
    answer(state, desk, &next, &catalog, notices)
}

fn name_of(section: &str) -> &'static str {
    match section {
        "pings" => "pings",
        "watching" => "watching",
        "chatbot" => "chatbot",
        "notifications" => "notifications",
        "self_service" => "self_service",
        "persona" => "persona",
        _ => "models",
    }
}

/// Validate, persist, then swap the live snapshot; any failure leaves the
/// saved selection and the last good snapshot as they were.
async fn switch_persona(desk: &ConfigDesk, active: &str, section: Section) -> Result<(), Refusal> {
    let files = desk
        .personas
        .as_ref()
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    let id = PersonaId::parse(active)
        .map_err(|_| Refusal::invalid("No such persona in the catalog."))?;
    let (dir, personas, port) = (files.dir.clone(), files.store.clone(), desk.store.clone());
    let handle = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let root = PersonaRoot::open(&dir).map_err(ReloadError::Invalid)?;
        personas.reload(&root, &id, |_| handle.block_on(port.save(section)))
    })
    .await
    .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    match outcome {
        Ok(_) => Ok(()),
        Err(ReloadError::Invalid(_)) => Err(Refusal::invalid(
            "That persona is not in the catalog or its files do not validate; nothing changed.",
        )),
        Err(ReloadError::Persist(error)) => Err(stored(error)),
    }
}

/// Re-reads the persona files for the persona in use (else the saved one,
/// else the tracked fallback) and swaps the snapshot.
async fn reload_profiles(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    let files = desk
        .personas
        .as_ref()
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    // Held so a concurrent persona switch cannot be undone by this reload.
    let current = desk.lock().await;
    let id = files
        .store
        .pin()
        .provenance()
        .effective
        .clone()
        .or_else(|| PersonaId::parse(&current.persona.active).ok())
        .or_else(|| PersonaId::parse(FALLBACK_PERSONA).ok())
        .ok_or_else(|| Refusal::from(ApiError::UNAVAILABLE))?;
    let (dir, personas) = (files.dir.clone(), files.store.clone());
    let outcome = tokio::task::spawn_blocking(move || {
        let root = PersonaRoot::open(&dir).map_err(ReloadError::Invalid)?;
        personas.reload(&root, &id, |_| Ok::<(), Infallible>(()))
    })
    .await
    .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    drop(current);
    if outcome.is_err() {
        return Err(Refusal::invalid(
            "The persona files do not validate; the last good profiles stay in use.",
        ));
    }
    let snapshot = files.store.pin();
    let (reloaded, skipped) = snapshot.active().map_or((0, 0), |active| {
        (
            active.profiles.readable.len(),
            active.profiles.unreadable.len(),
        )
    });
    let mut message = format!(
        "Reloaded {reloaded} reply profile{} from config/personas/profiles/.",
        if reloaded == 1 { "" } else { "s" }
    );
    if skipped > 0 {
        message.push_str(&format!(" {skipped} unreadable file(s) were skipped."));
    }
    Ok(Json(json!({ "message": message, "reloaded": reloaded })).into_response())
}
