//! Runtime settings (A9): `GET`/`PATCH /api/admin/config` and the profile
//! reload. One lock serialises saves; each saved change is published on
//! [`ConfigDesk::subscribe`]. `PATCH` honours `Idempotency-Key` with a
//! stored replay (scope `config`, written in the save's transaction): a
//! replay answers the current view with the first request's notices; the
//! same key for another body is `422 idempotency_mismatch`; the reload is
//! naturally repeatable.

mod access;
mod changes;
mod desk;
mod models;
mod patch;
mod stack;

#[cfg(test)]
pub(crate) use access::{AccessReport, AccessRow};

use std::{convert::Infallible, sync::Arc};

use axum::{
    Json, Router,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};

pub use desk::{
    CatalogRead, ConfigDesk, ConfigFacts, ConfigFuture, ConfigInputs, LiveProfileChoices,
    ModelCatalog, PersonaFiles, SettingsChanged, SettingsPort,
};

use super::{
    replay::Keyed,
    write::{Refusal, bad_body, origin, state},
};
use crate::{
    api::{auth::AdminSession, error::ApiError, listeners::Site, state::ApiState},
    chat::persona::{FALLBACK_PERSONA, PersonaId, PersonaRoot, ProfileId, ReloadError},
    domain::{
        history::Surface,
        settings::{
            LOCAL_CONTEXT_WARNING, RuntimeSettings, Section, SettingsChange, SettingsError,
            diff_rows,
        },
    },
    infrastructure::store::replays::{ReplayScope, StoredReplay},
};

/// The stored body of a keyed PATCH: the notices it answered with.
#[derive(Deserialize)]
struct Notices {
    notices: Vec<String>,
}

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
        Section::Profanity(value) => settings.profanity = value,
        Section::Schedule(value) => settings.schedule = value,
        Section::Posting(value) => settings.posting = value,
        Section::IdList(list, ids) => *list.get_mut(settings) = ids,
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
    let saved = desk.saved_lists().await;
    let mut view = desk.view(
        settings,
        catalog,
        &channels,
        &roles,
        notices,
        last_digest,
        &saved,
    );
    fill_in_use(state, &mut view.models.groups);
    Ok(Json(view).into_response())
}

/// Each row's in-flight permits from the governor snapshot the Limits page
/// reads; rows stay `null` without model serving or a group it does not run.
fn fill_in_use(state: &ApiState, groups: &mut [crate::api::dto::config::CapacityGroup]) {
    let Some(limits) = &state.model_limits else {
        return;
    };
    let live: std::collections::BTreeMap<String, u32> = limits(state.now())
        .into_iter()
        .map(|group| (group.name, group.permits.in_use))
        .collect();
    for row in groups {
        row.in_use = live.get(&row.group).copied();
    }
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
    let now = state.now();
    // The whole body is the identity; serde_json maps are sorted, so key
    // order never changes it.
    let keyed = key.map(|key| Keyed::new(ReplayScope::Config, &session, key, &body.to_string()));

    // The settings lock also serialises same-key PATCHes.
    let mut current = desk.lock().await;
    if let Some(keyed) = &keyed {
        let found = desk
            .store
            .replay(keyed.actor().to_owned(), keyed.key().to_owned(), now)
            .await
            .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?;
        if let Some(found) = keyed.check(found)? {
            let notices = serde_json::from_str::<Notices>(&found.body)
                .map_err(|_| Refusal::from(ApiError::UNAVAILABLE))?
                .notices;
            let settings = current.clone();
            drop(current);
            let catalog = desk.catalog().await;
            return answer(state, desk, &settings, &catalog, notices).await;
        }
    }

    let (name, fields) = patch::section(&body)?;
    let switch_active_persona = name == "persona" && fields.contains_key("active");
    let mut notices = Vec::new();
    let mut context_warnings = Vec::new();
    let mut catalog = None;
    let list = patch::id_list(name, fields)?;
    let section = match name {
        _ if let Some(list) = list => {
            let ids = patch::ids(
                fields.values().next().expect("a list body has one key"),
                &format!(
                    "{}.{}",
                    list.section(),
                    fields.keys().next().expect("one key")
                ),
            )?;
            Section::IdList(list, ids)
        }
        "pings" => Section::Pings(patch::pings(&current.pings, fields)?),
        "watching" => Section::Watching(patch::watching(&current.watching, fields)?),
        "chatbot" => Section::Chatbot(patch::chatbot(
            &current.chatbot,
            fields,
            &desk.missing_env(&current),
        )?),
        "notifications" => {
            Section::Notifications(patch::notifications(&current.notifications, fields)?)
        }
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
        "profanity" => Section::Profanity(patch::profanity(&current.profanity, fields)?),
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
    // History's record of this save, written with its rows: a refused save or
    // one that changes no stored row records nothing.
    let values = diff_rows(&current, &next);
    let record = (!values.is_empty()).then(|| SettingsChange {
        id: 0,
        at: state.now(),
        actor: session.actor.clone(),
        surface: Surface::AdminPortal,
        section: name_of(name).to_owned(),
        revision: desk.next_revision(),
        values,
    });
    // A keyed PATCH's answer commits with the save; post-apply notices
    // (the models switch below) then update it.
    let answered = |notices: &[String]| {
        keyed
            .as_ref()
            .map(|keyed| keyed.answered(StatusCode::OK, &json!({ "notices": notices }), now))
    };
    let saved_replay = answered(&notices);
    let mut replay_saved = false;
    if switch_active_persona {
        let Section::Persona(persona) = &section else {
            unreachable!("active persona patch makes a persona section")
        };
        switch_persona(
            desk,
            &persona.active,
            section.clone(),
            record,
            saved_replay.clone(),
        )
        .await?;
        replay_saved = true;
    } else if next != *current {
        let list = match &section {
            Section::IdList(list, _) => Some(*list),
            _ => None,
        };
        desk.store
            .save(section, record, saved_replay.clone())
            .await
            .map_err(stored)?;
        replay_saved = true;
        if let Some(list) = list {
            desk.note_saved(list);
        }
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
    // A no-op PATCH saves nothing, so its answer is written alone; a saved
    // one is updated only when the models switch added notices. The save
    // (and the live switch) already happened, so a failed write is logged
    // and the answer still goes out.
    if let Some(replay) = answered(&notices)
        && (!replay_saved || saved_replay.as_ref() != Some(&replay))
        && desk.store.put_replay(replay).await.is_err()
    {
        // Store error text may carry paths, so only the event is logged.
        crate::runtime::logging::event("WARN", "idempotency_replay_unrecorded", json!({}));
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
        "profanity" => "profanity",
        _ => "models",
    }
}

/// Validate, persist, then swap the live snapshot; any failure leaves the
/// saved selection and the last good snapshot as they were.
async fn switch_persona(
    desk: &ConfigDesk,
    active: &str,
    section: Section,
    record: Option<SettingsChange>,
    replay: Option<StoredReplay>,
) -> Result<(), Refusal> {
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
        personas.reload(&root, &id, |_| {
            handle.block_on(port.save(section, record, replay))
        })
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
