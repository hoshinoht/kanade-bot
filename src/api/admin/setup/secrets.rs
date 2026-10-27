//! Admin Setup's saved secrets: which are set and where (never a value),
//! checking a candidate against its service, saving one into the saved
//! directory (write-only, after a fresh sign-in), and "Restart now", which
//! applies them. A host file always wins: Setup reports it and refuses to
//! save over it.

use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::Deserialize;

use crate::{
    api::{
        admin::write::{Refusal, bad_body, state},
        auth::AdminSession,
        dto::setup::{
            SecretCheck, SecretCheckResult, SecretName, SecretSource, SetupSecret, SetupSecrets,
        },
        error::ApiError,
        listeners::Site,
        state::{ApiState, SecretDesk},
    },
    runtime::secrets::{SaveError, Source},
};
use kanade_domain::settings::{RowDiff, SettingsChange};

pub fn routes() -> Router<Arc<Site>> {
    Router::new()
        .route("/api/admin/setup/secrets", get(list))
        .route("/api/admin/setup/secrets/{name}", put(save))
        .route("/api/admin/setup/secrets/{name}/check", post(check))
        .route("/api/admin/setup/restart", post(restart))
}

/// A check ends by then, so the dialog never hangs on a slow service.
const CHECK_DEADLINE: Duration = Duration::from_secs(10);
/// The History settings section of a saved secret (no settings row).
const SECTION: &str = "secrets";

type Reply = Result<Response, Refusal>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    value: String,
}

fn desk(state: &ApiState) -> Result<&SecretDesk, Refusal> {
    state.setup_secrets.as_deref().ok_or_else(|| {
        Refusal::conflict(
            "not_saveable",
            "This server has no saved secrets directory; put secrets in the secrets directory.",
        )
    })
}

fn name(raw: &str) -> Result<SecretName, Refusal> {
    SecretName::parse(raw).ok_or_else(|| {
        Refusal::not_found("not_found", "Setup cannot save a secret with that name.")
    })
}

fn view(desk: &SecretDesk) -> Result<SetupSecrets, Refusal> {
    let secrets = SecretName::ALL
        .iter()
        .filter_map(|&name| desk.file(name.as_str()).map(|file| (name, file)))
        .map(|(name, file)| {
            let source = match file.source().map_err(|_| ApiError::UNAVAILABLE)? {
                Source::Absent => SecretSource::Absent,
                Source::Host => SecretSource::Host,
                Source::Saved => SecretSource::Saved,
            };
            Ok(SetupSecret {
                name,
                source,
                checkable: matches!(name, SecretName::DiscordToken | SecretName::ModelApiKey),
            })
        })
        .collect::<Result<_, Refusal>>()?;
    Ok(SetupSecrets {
        secrets,
        restart_pending: desk.restart_pending(),
        restart_available: desk.restart.is_some(),
    })
}

async fn list(State(site): State<Arc<Site>>, _: AdminSession) -> Reply {
    let state = state(&site)?;
    Ok(Json(view(desk(state)?)?).into_response())
}

async fn check(
    State(site): State<Arc<Site>>,
    _: AdminSession,
    Path(raw): Path<String>,
    body: Result<Json<Candidate>, JsonRejection>,
) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    let name = name(&raw)?;
    let Json(candidate) = body.map_err(bad_body)?;
    let result = tokio::time::timeout(CHECK_DEADLINE, desk.checks.check(name, &candidate.value))
        .await
        .unwrap_or_else(|_| SecretCheck::new(SecretCheckResult::Unreachable));
    Ok(Json(result).into_response())
}

async fn save(
    State(site): State<Arc<Site>>,
    session: AdminSession,
    Path(raw): Path<String>,
    body: Result<Json<Candidate>, JsonRejection>,
) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    let name = name(&raw)?;
    let Json(candidate) = body.map_err(bad_body)?;
    session.require_fresh(state.now())?;
    let file = desk.file(name.as_str()).ok_or_else(|| {
        Refusal::conflict(
            "not_saveable",
            "This secret can only be a file in the secrets directory.",
        )
    })?;
    let before = file.source().map_err(|_| ApiError::UNAVAILABLE)?;
    file.save(&candidate.value).map_err(|error| match error {
        SaveError::OnHost => Refusal::conflict("on_host", error.to_string()),
        SaveError::NotSaveable => Refusal::conflict("not_saveable", error.to_string()),
        SaveError::Invalid => Refusal::unprocessable("invalid", error.to_string()),
        SaveError::Unavailable => ApiError::UNAVAILABLE.into(),
    })?;
    desk.mark_saved(file.name());
    let origin = session.origin();
    let change = SettingsChange {
        id: 0,
        at: state.now(),
        actor: origin.actor,
        surface: origin.surface,
        section: SECTION.into(),
        revision: 0,
        values: [(
            format!("secret.{}", name.as_str()),
            RowDiff {
                from: if before == Source::Saved {
                    "saved"
                } else {
                    "not set"
                }
                .into(),
                to: if before == Source::Saved {
                    "replaced"
                } else {
                    "saved"
                }
                .into(),
            },
        )]
        .into(),
    };
    // The file is written; a History row that fails to land is only logged.
    if state
        .store
        .record_settings_change(change, None)
        .await
        .is_err()
    {
        kanade_log::log!(warn, "secret_save_unrecorded", secret = name.as_str());
    }
    kanade_log::log!(info, "secret_saved", secret = name.as_str());
    Ok(Json(view(desk)?).into_response())
}

async fn restart(State(site): State<Arc<Site>>, session: AdminSession) -> Reply {
    let state = state(&site)?;
    let desk = desk(state)?;
    session.require_fresh(state.now())?;
    let restart = desk.restart.as_ref().ok_or_else(|| {
        Refusal::conflict(
            "not_supervised",
            "This server is not run by a supervisor that would start it again; restart it yourself.",
        )
    })?;
    restart().map_err(|message| Refusal::unprocessable("would_fail", message))?;
    Ok(StatusCode::ACCEPTED.into_response())
}
