//! Operator log lines for saved settings and persona swaps. Every settings row
//! is a structured value (ids, flags, counts, clocks, aliases), never free
//! text or a secret, so before/after values are safe to log.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::models::LocalContextWarning;
use crate::{
    chat::persona::PersonaSnapshot,
    domain::settings::{LOCAL_CONTEXT_WARNING, RuntimeSettings, Section, section_rows},
    infrastructure::llm::setup::{RoleSwap, RunningRole},
    runtime::logging,
};

fn rows(settings: &RuntimeSettings) -> BTreeMap<&'static str, String> {
    [
        Section::Pings(settings.pings.clone()),
        Section::Watching(settings.watching.clone()),
        Section::Chatbot(settings.chatbot.clone()),
        Section::Notifications(settings.notifications),
        Section::SelfService(settings.self_service),
        Section::Persona(settings.persona.clone()),
        Section::Models(settings.models.clone()),
        Section::Schedule(settings.schedule),
        Section::Posting(settings.posting.clone()),
    ]
    .iter()
    .flat_map(section_rows)
    .collect()
}

/// `{key: {"from", "to"}}` for every stored row that differs.
pub(super) fn diff(before: &RuntimeSettings, after: &RuntimeSettings) -> Map<String, Value> {
    let (before, after) = (rows(before), rows(after));
    after
        .iter()
        .filter(|(key, value)| before.get(*key) != Some(*value))
        .map(|(key, value)| {
            let from = before.get(key).cloned().unwrap_or_default();
            ((*key).to_owned(), json!({"from": from, "to": value}))
        })
        .collect()
}

pub(super) fn settings_changed(
    revision: u64,
    section: &str,
    actor_kind: &str,
    before: &RuntimeSettings,
    after: &RuntimeSettings,
) {
    let values = diff(before, after);
    let keys: Vec<&String> = values.keys().collect();
    logging::event(
        "INFO",
        "settings_changed",
        json!({
            "revision": revision,
            "section": section,
            "keys": keys,
            "values": values,
            "actor_kind": actor_kind,
            "surface": "admin_portal",
        }),
    );
}

/// One line per role a saved context change leaves past the local warning
/// threshold (the save itself is never blocked).
pub(super) fn local_context_warnings(warnings: &[LocalContextWarning]) {
    for warning in warnings {
        logging::event(
            "WARN",
            "model_warning",
            json!({
                "kind": "local_context",
                "role": warning.role,
                "alias": warning.alias,
                "window": warning.window,
                "message": LOCAL_CONTEXT_WARNING,
                "surface": "admin_portal",
            }),
        );
    }
}

fn running(running: Option<&RunningRole>) -> Value {
    running.map_or(Value::Null, |running| {
        json!({
            "alias": running.alias,
            "effort": running.effort.map(|effort| effort.as_str()),
        })
    })
}

/// One line per role whose running alias or effort changed; the next
/// session of that role opens with `to`.
pub(super) fn models_applied(swaps: &[RoleSwap]) {
    for swap in swaps {
        logging::event(
            "INFO",
            "model_role_switched",
            json!({
                "role": swap.role.as_str(),
                "from": running(swap.before.as_ref()),
                "to": running(swap.after.as_ref()),
            }),
        );
    }
}

pub(super) fn models_apply_failed(error: &str) {
    logging::event("WARN", "model_roles_not_applied", json!({"error": error}));
}

fn profile_count(snapshot: &PersonaSnapshot) -> usize {
    snapshot
        .active()
        .map_or(0, |active| active.profiles.readable.len())
}

fn effective(snapshot: &PersonaSnapshot) -> Option<String> {
    snapshot
        .provenance()
        .effective
        .as_ref()
        .map(ToString::to_string)
}

pub(super) fn persona_switched(from: &PersonaSnapshot, to: &PersonaSnapshot) {
    logging::event(
        "INFO",
        "persona_switched",
        json!({
            "from": effective(from),
            "to": effective(to),
            "profiles": profile_count(to),
        }),
    );
}

pub(super) fn personas_reloaded(snapshot: &PersonaSnapshot) {
    let issues: Vec<Value> = snapshot.active().map_or_else(Vec::new, |active| {
        active
            .profiles
            .unreadable
            .iter()
            .map(|issue| json!({"file": issue.basename, "error": issue.error.to_string()}))
            .collect()
    });
    logging::event(
        if issues.is_empty() { "INFO" } else { "WARN" },
        "personas_reloaded",
        json!({
            "persona": effective(snapshot),
            "profiles": profile_count(snapshot),
            "issues": issues,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_changed_names_keys_and_persona_before_after() {
        logging::capture();
        let before = RuntimeSettings::default();
        let mut after = before.clone();
        after.persona.active = "aria".into();
        after.chatbot.enabled = !before.chatbot.enabled;
        settings_changed(7, "persona", "admin", &before, &after);
        let line = logging::captured().remove(0);
        assert_eq!(line["event"], "settings_changed");
        assert_eq!(line["revision"], 7);
        assert_eq!(line["surface"], "admin_portal");
        assert_eq!(line["actor_kind"], "admin");
        let keys: Vec<&str> = line["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|key| key.as_str().unwrap())
            .collect();
        assert!(keys.contains(&"persona"), "{keys:?}");
        assert!(keys.contains(&"chat_mode"), "{keys:?}");
        assert_eq!(keys.len(), 2);
        assert_eq!(line["values"]["persona"]["from"], before.persona.active);
        assert_eq!(line["values"]["persona"]["to"], "aria");
    }

    #[test]
    fn a_context_change_logs_its_row_before_and_after_and_local_warnings() {
        logging::capture();
        let before = RuntimeSettings::default();
        let mut after = before.clone();
        after.models.context.chat.cap = Some(32_768);
        settings_changed(3, "models", "admin", &before, &after);
        local_context_warnings(&[LocalContextWarning {
            role: "chat",
            alias: "kanata/chat".into(),
            window: 32_768,
        }]);
        let lines = logging::captured();
        let changed = &lines[0];
        assert_eq!(changed["keys"], serde_json::json!(["v5.model_context"]));
        let row = &changed["values"]["v5.model_context"];
        let from: Value = serde_json::from_str(row["from"].as_str().unwrap()).unwrap();
        let to: Value = serde_json::from_str(row["to"].as_str().unwrap()).unwrap();
        assert!(from["chat"].get("cap").is_none());
        assert_eq!(to["chat"]["cap"], 32_768);
        let warning = &lines[1];
        assert_eq!(warning["level"], "WARN");
        assert_eq!(warning["event"], "model_warning");
        assert_eq!(warning["kind"], "local_context");
        assert_eq!(warning["role"], "chat");
        assert_eq!(warning["window"], 32_768);
        assert_eq!(warning["message"], LOCAL_CONTEXT_WARNING);
    }

    #[test]
    fn unchanged_settings_have_no_diff() {
        let settings = RuntimeSettings::default();
        assert!(diff(&settings, &settings).is_empty());
    }
}
