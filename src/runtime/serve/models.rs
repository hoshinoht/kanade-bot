//! Model role seeds and the startup model report: env seeds fill roles with no
//! saved row, then one background `check_startup` is logged as JSON lines.

use std::{collections::BTreeMap, sync::Arc};

use serde_json::json;
use tokio::task::JoinHandle;

use crate::{
    domain::settings::{
        ContextSettings, LOCAL_CONTEXT_WARNING, MAX_CONTEXT_TOKENS, RuntimeSettings, keys,
    },
    infrastructure::llm::{
        governor::{ConfigWarning, Role},
        setup::{
            Listing, ModelRoles, ModelStack, RoleEffort, StartupReport, StartupWarning,
            resolve_context,
        },
    },
    runtime::{config::ModelSettings, logging},
};

/// Where a role's reasoning setting came from (`model_role.source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Stored,
    Env,
    Default,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stored => "stored",
            Self::Env => "env",
            Self::Default => "default",
        }
    }
}

pub type Sources = BTreeMap<Role, Source>;

/// Row → env seed → default for each role's alias and reasoning.
pub fn seed_roles(
    settings: &mut RuntimeSettings,
    models: &ModelSettings,
    stored: &BTreeMap<String, String>,
) -> Sources {
    let roles = &mut settings.models;
    let mut sources = Sources::new();
    for (role, slot, alias, reasoning, key) in [
        (
            Role::Extraction,
            &mut roles.extraction,
            &models.extract_model,
            models.extract_reasoning,
            keys::EXTRACT_REASONING,
        ),
        (
            Role::Chat,
            &mut roles.chat,
            &models.chat_model,
            models.chat_reasoning,
            keys::CHAT_REASONING,
        ),
        (
            Role::Rewrite,
            &mut roles.rewrite,
            &models.rewrite_model,
            models.rewrite_reasoning,
            keys::REWRITE_REASONING,
        ),
    ] {
        if slot.alias.is_none() {
            slot.alias.clone_from(alias);
        }
        let source = match (stored.contains_key(key), reasoning) {
            (true, _) => Source::Stored,
            (false, Some(level)) => {
                slot.reasoning = level;
                Source::Env
            }
            (false, None) => Source::Default,
        };
        sources.insert(role, source);
    }
    if !stored.contains_key(keys::MODEL_CONTEXT)
        && let Some(context) = &models.context
    {
        settings.models.context = context.clone();
        if !models.context_clamped.is_empty() {
            logging::event(
                "WARN",
                "model_context_clamped",
                json!({
                    "fields": models.context_clamped
                        .iter()
                        .map(|path| format!("models.context.{path}"))
                        .collect::<Vec<_>>(),
                    "max": MAX_CONTEXT_TOKENS,
                }),
            );
        }
    }
    sources
}

/// Aborts the model background tasks when serve drops it at shutdown.
#[derive(Debug, Default)]
pub struct ModelTasks(Vec<JoinHandle<()>>);

impl ModelTasks {
    #[cfg(test)]
    pub async fn report_done(&mut self) {
        if let Some(report) = self.0.pop() {
            let _ = report.await;
        }
    }
}

impl Drop for ModelTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

/// The catalog refresh plus one startup report; neither blocks HTTP readiness.
pub fn start(
    stack: Option<&Arc<ModelStack>>,
    sources: Sources,
    context: ContextSettings,
) -> ModelTasks {
    let Some(stack) = stack else {
        logging::event("INFO", "models_disabled", json!({}));
        return ModelTasks::default();
    };
    let refresh = stack.spawn_catalog_refresh();
    let stack = stack.clone();
    let report = tokio::spawn(async move {
        let report = stack.check_startup().await;
        for (level, event, fields) in lines(&stack, &report, &sources, &context) {
            logging::event(level, event, fields);
        }
    });
    ModelTasks(vec![refresh, report])
}

type Line = (&'static str, &'static str, serde_json::Value);

/// Never includes the key or the gateway URL.
fn lines(
    stack: &ModelStack,
    report: &StartupReport,
    sources: &Sources,
    context: &ContextSettings,
) -> Vec<Line> {
    let mut out = vec![match &report.listing {
        Listing::Listed { models } => ("INFO", "models_listed", json!({"models": models})),
        Listing::Degraded { reason_code } => (
            "WARN",
            "models_degraded",
            json!({"reason": reason_code, "routes": "external until a listing succeeds"}),
        ),
    }];
    let catalog = stack.catalog();
    let efforts = stack.efforts();
    for role in ModelRoles::ALL {
        let Some(route) = stack.governor.route(role) else {
            continue;
        };
        let status = efforts.get(&role);
        let source = if catalog.variant(&route.alias).is_some() {
            "fixed"
        } else if status.is_some_and(|status| status.stranded.is_some()) {
            "floor"
        } else if stack.roles().get(role).effort == RoleEffort::Inherit {
            "inherit"
        } else {
            sources
                .get(&role)
                .copied()
                .unwrap_or(Source::Default)
                .as_str()
        };
        let route_kind = stack.route_kind(role).unwrap_or("homelab");
        out.push((
            "INFO",
            "model_role",
            json!({
                "role": role.as_str(),
                "alias": route.alias,
                "effort": status.map(|status| status.effort.as_str()),
                "source": source,
                "route": route_kind,
            }),
        ));
    }
    for warning in &report.warnings {
        let (level, kind) = match warning {
            StartupWarning::ExternalUnmasked { .. } => ("WARN", "external_unmasked"),
            StartupWarning::UnpublishedEffort { .. } => ("INFO", "unpublished_effort"),
            StartupWarning::Governor(ConfigWarning::PermitsAboveGateway { .. }) => {
                ("WARN", "capacity")
            }
            StartupWarning::Governor(ConfigWarning::UngroupedRole { .. }) => ("WARN", "ungrouped"),
        };
        out.push((
            level,
            "model_warning",
            json!({"kind": kind, "message": warning.to_string()}),
        ));
    }
    // Once per role whose reserve leaves no prompt room, and once per role
    // whose local route runs past the warning threshold (saved, never
    // blocked, so this is the operator's only signal).
    for role in ModelRoles::ALL {
        let Some(route) = stack.governor.route(role) else {
            continue;
        };
        let resolved = resolve_context(context, &catalog, role, &route.alias);
        if resolved.reserve_fills_window() {
            out.push((
                "WARN",
                "model_warning",
                json!({
                    "kind": "context_reserve",
                    "role": role.as_str(),
                    "alias": route.alias,
                    "window": resolved.window,
                    "reserve": resolved.reserve,
                    "message": resolved.reserve_warning(role, &route.alias),
                }),
            ));
        }
        if resolved.local_warning {
            out.push((
                "WARN",
                "model_warning",
                json!({
                    "kind": "local_context",
                    "role": role.as_str(),
                    "alias": route.alias,
                    "window": resolved.window,
                    "message": LOCAL_CONTEXT_WARNING,
                }),
            ));
        }
    }
    out
}
