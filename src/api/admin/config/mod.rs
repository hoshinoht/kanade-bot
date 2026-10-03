//! Runtime settings (A9): `GET`/`PATCH /api/admin/config` and the profile
//! reload. One lock serialises saves; each saved change is published on
//! [`ConfigDesk::subscribe`]. `PATCH` honours `Idempotency-Key` (a replay
//! answers the current view with the first request's notices; the same key
//! for another body is `422 idempotency_mismatch`); the reload is naturally
//! repeatable.

mod access;
mod changes;
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
    CatalogRead, ConfigDesk, ConfigFacts, ConfigFuture, ConfigInputs, LiveProfileChoices,
    ModelCatalog, PersonaFiles, SettingsChanged, SettingsPort,
};

use super::write::{Refusal, bad_body, origin, state};
use crate::{
    api::{auth::AdminSession, error::ApiError, listeners::Site, state::ApiState},
    chat::persona::{FALLBACK_PERSONA, PersonaId, PersonaRoot, ProfileId, ReloadError},
    domain::settings::{LOCAL_CONTEXT_WARNING, RuntimeSettings, Section, SettingsError},
};

type Reply = Result<axum::response::Response, Refusal>;

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/config", get(read).patch(update))
        .route("/api/admin/config/profiles/reload", post(reload_profiles))
        .route("/api/admin/access", get(access::read))
        .route("/api/admin/access/recheck", post(access::recheck))
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

fn role_profiles_conflict() -> Refusal {
    Refusal::new(
        StatusCode::CONFLICT,
        "conflict",
        "Reply profile assignments changed. Reload Config before saving again.",
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
        Section::RunLengths(value) => settings.run_lengths = value,
        Section::Schedule(value) => settings.schedule = value,
        Section::Posting(value) => settings.posting = value,
    }
}

async fn answer(
    state: &ApiState,
    desk: &ConfigDesk,
    settings: &RuntimeSettings,
    catalog: &CatalogRead,
    notices: Vec<String>,
) -> Reply {
    let roles = if state.channels.connected() {
        state.channels.roles()
    } else {
        Vec::new()
    };
    let channels = state.channels.channels();
    let last_digest = last_digest(state, &channels).await;
    let view = desk.view(settings, catalog, &channels, &roles, notices, last_digest);
    Ok(Json(view).into_response())
}

/// A failed journal read leaves the field empty rather than failing the page.
async fn last_digest(
    state: &ApiState,
    channels: &[crate::api::state::ChannelEntry],
) -> Option<crate::api::dto::config::LastDigest> {
    let digests = match state.store.digests().await {
        Ok(digests) => digests,
        // Store error text may carry paths, so only the event is logged.
        Err(_) => {
            crate::runtime::logging::event(
                "WARN",
                "config_digest_unreadable",
                serde_json::json!({}),
            );
            return None;
        }
    };
    let current = state.policy.week_of(&state.now()).ok()?.to_fixed().to_utc();
    crate::api::dto::config::last_digest(
        &digests,
        state.policy.zone(),
        current,
        channels,
        state.guild_id.as_deref(),
    )
}

async fn read(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    let settings = desk.settings().await;
    let catalog = desk.catalog().await;
    answer(state, desk, &settings, &catalog, Vec::new()).await
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
        return answer(state, desk, &settings, &catalog, entry.notices).await;
    }

    let (name, fields) = patch::section(&body)?;
    let switch_active_persona = name == "persona" && fields.contains_key("active");
    let mut notices = Vec::new();
    let mut context_warnings = Vec::new();
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
        "persona" => {
            if fields.contains_key("role_profiles") {
                let (assignments, expected) = patch::role_profiles(fields)?;
                if expected
                    != crate::api::dto::config::role_profiles_digest(&current.persona.role_profiles)
                {
                    return Err(role_profiles_conflict());
                }
                let changed: Vec<_> = assignments
                    .iter()
                    .filter(|assignment| {
                        !current.persona.role_profiles.iter().any(|saved| {
                            saved.role_id == assignment.role_id
                                && saved.profile == assignment.profile
                        })
                    })
                    .collect();
                let choices = desk.profile_choices_for(&current);
                for assignment in &changed {
                    let profile = ProfileId::parse(&assignment.profile).map_err(|_| {
                        patch::PatchError::invalid("Pick a readable reply profile.")
                    })?;
                    if !choices.readable.contains(&profile) {
                        return Err(
                            patch::PatchError::invalid("Pick a readable reply profile.").into()
                        );
                    }
                }
                if !changed.is_empty() {
                    if !state.channels.connected() {
                        return Err(ApiError::UNAVAILABLE.into());
                    }
                    let roles = state.channels.roles();
                    if changed
                        .iter()
                        .any(|assignment| !roles.iter().any(|role| role.id == assignment.role_id))
                    {
                        return Err(
                            patch::PatchError::invalid("Pick a current Discord role.").into()
                        );
                    }
                }
                let mut persona = current.persona.clone();
                persona.role_profiles = assignments;
                Section::Persona(persona)
            } else {
                let choices = desk.profile_choices_for(&current);
                Section::Persona(patch::persona(&current.persona, fields, &choices.readable)?)
            }
        }
        "run_lengths" => Section::RunLengths(patch::run_lengths(
            &current.run_lengths,
            fields,
            &state.catalog,
        )?),
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
            let mut next = current.models.clone();
            if let Some(roles) = fields.get("roles") {
                let (roles, reset) = models::apply_roles(&next, roles, &read.snapshot)?;
                next = roles;
                notices.extend(reset);
            }
            if let Some(context) = fields.get("context") {
                next.context = patch::context(context)?;
            }
            context_warnings = models::validate_context(&next.context, &next, &read.snapshot)?;
            if !context_warnings.is_empty() {
                notices.push(LOCAL_CONTEXT_WARNING.into());
            }
            let ungrouped = models::ungrouped(&current.models, &next, &desk.facts.model_groups);
            if !ungrouped.is_empty() {
                return Err(Refusal::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "ungrouped",
                    ungrouped.join(" "),
                ));
            }
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
            catalog = Some(read);
            Section::Models(next)
        }
    };

    let mut next = current.clone();
    put(&mut next, section.clone());
    if current.persona.profile_visibility != next.persona.profile_visibility {
        notices.push("Reply profile visibility updated.".into());
    }
    if switch_active_persona {
        let Section::Persona(persona) = &section else {
            unreachable!("active persona patch makes a persona section")
        };
        switch_persona(desk, &persona.active, section.clone()).await?;
    } else if next != *current {
        desk.store.save(section).await.map_err(stored)?;
    }
    let saved_before = current.models.clone();
    if next != *current {
        let before = current.clone();
        *current = next.clone();
        let revision = desk.publish(name_of(name), actor.clone(), &next);
        changes::settings_changed(revision, name_of(name), &actor, &before, &next);
        if before.models.context != next.models.context {
            changes::local_context_warnings(&context_warnings);
        }
    }
    // Every models save re-applies, so a stack left behind catches up.
    if let ("models", Some(stack)) = (name_of(name), &desk.models) {
        match stack.apply(&next.models) {
            Ok(swaps) => {
                changes::models_applied(&swaps);
                notices.extend(models::awaiting_restart(
                    &saved_before,
                    &next.models,
                    &stack.awaiting_restart(),
                ));
            }
            Err(error) => {
                changes::models_apply_failed(&error);
                notices.push(
                    "Saved, but the running models could not switch; they apply when the bot restarts."
                        .into(),
                );
            }
        }
    }
    if let Some(key) = key {
        desk.remember(desk::Remembered {
            actor,
            key,
            digest,
            notices: notices.clone(),
        });
    }
    drop(current);
    let catalog = match catalog {
        Some(catalog) => catalog,
        None => desk.catalog().await,
    };
    answer(state, desk, &next, &catalog, notices).await
}

fn name_of(section: &str) -> &'static str {
    match section {
        "pings" => "pings",
        "watching" => "watching",
        "chatbot" => "chatbot",
        "notifications" => "notifications",
        "self_service" => "self_service",
        "persona" => "persona",
        "run_lengths" => "run_lengths",
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
    let before = files.store.pin();
    let handle = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        let root = PersonaRoot::open(&dir).map_err(ReloadError::Invalid)?;
        personas.reload(&root, &id, |_| handle.block_on(port.save(section)))
    })
    .await
    .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
    match outcome {
        Ok(_) => {
            changes::persona_switched(&before, &files.store.pin());
            Ok(())
        }
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
    changes::personas_reloaded(&snapshot);
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
