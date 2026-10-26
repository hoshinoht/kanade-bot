//! The Config page's view of the desk: saved settings, catalog and facts as
//! the wire shape.

use super::models;
use crate::{
    api::dto::config::{self as dto, ConfigView, EnvRow, KeyLimits, ManageMessages},
    app::{
        config::{CatalogRead, ConfigDesk},
        guild::RoleEntry,
    },
    infrastructure::llm::setup::CapacityGroup,
};
use kanade_domain::settings::{IdList, RuntimeSettings, keys};

/// `saved` when `key` has a stored row, else `env`.
fn seeded_source(saved: &[&str], key: &str) -> &'static str {
    if saved.contains(&key) { "saved" } else { "env" }
}

/// The Config page for `settings` as the desk and catalog see them now.
pub(super) fn view(
    desk: &ConfigDesk,
    settings: &RuntimeSettings,
    catalog: &CatalogRead,
    roles: &[RoleEntry],
    notices: Vec<String>,
    last_digest: Option<dto::LastDigest>,
    saved: &[&'static str],
) -> ConfigView {
    let source = |list: IdList| seeded_source(saved, list.key());
    let missing_env = desk.missing_env();
    let choices = desk.profile_choices_for(settings);
    let persona = dto::persona(
        &settings.persona.active,
        choices.snapshot.as_deref(),
        &choices.selectable,
        &settings.persona.role_profiles,
        roles,
    );
    let groups = groups(desk, settings);
    let declared = !desk.facts.model_groups.is_empty();
    ConfigView {
        pings: dto::pings(settings),
        watching: dto::Watching {
            paused: settings.watching.paused,
            extract_enabled: settings.watching.extract_enabled,
            channel_ids: settings.watching.channel_ids.clone(),
            channel_ids_source: source(IdList::WatchedChannels),
            category_ids: settings.watching.category_ids.clone(),
            category_ids_source: source(IdList::WatchedCategories),
        },
        chatbot: dto::Chatbot {
            enabled: settings.chatbot.enabled,
            configured: missing_env.is_empty(),
            missing_env,
            member_rate: settings.chatbot.member_rate.into(),
            guild_rate: settings.chatbot.guild_rate.into(),
            category_ids: settings.chatbot.category_ids.clone(),
            category_ids_source: source(IdList::ChatCategories),
        },
        notifications: dto::Notifications {
            quiet_mode: settings.notifications.quiet_mode,
            header_generation_time: crate::api::dto::hhmm(
                settings.notifications.header_generation_time,
            ),
            header_estimates: settings.notifications.header_estimates,
            header_teasing: settings.notifications.header_teasing,
        },
        self_service: dto::self_service(settings),
        posting: dto::Posting {
            channel_id: settings.posting.channel_id.clone(),
            channel_id_source: seeded_source(saved, keys::POST_CHANNEL),
        },
        persona,
        models: dto::Models {
            reachable: catalog.reachable,
            no_listing: catalog.no_listing,
            catalog: catalog
                .snapshot
                .models
                .iter()
                .map(|model| dto::model_info(model, &catalog.snapshot))
                .collect(),
            roles: dto::roles(settings, &catalog.snapshot, &desk.running()),
            groups: groups
                .iter()
                .flat_map(|group| {
                    group.aliases.iter().map(|alias| dto::CapacityGroup {
                        model: alias.clone(),
                        group: group.name.clone(),
                        permits: Some(group.permits),
                        // Filled from the live governor by the handler.
                        in_use: None,
                    })
                })
                .collect(),
            groups_source: if declared { "config" } else { "default" },
            alias_limits: dto::alias_limits(&catalog.snapshot),
            // Kanata publishes no per-key limit.
            key_limits: KeyLimits {
                max_in_flight: None,
                shared: true,
            },
            capacity_check: models::capacity(
                &settings.models,
                &groups,
                declared,
                catalog.reachable.then_some(&catalog.snapshot),
            ),
            // Retained for compatibility with the existing config schema.
            pii_pseudonymise: false,
            context: dto::context_settings(&settings.models.context),
        },
        run_lengths: dto::run_lengths(&settings.run_lengths),
        profanity: dto::profanity(&settings.profanity),
        manage_messages: ManageMessages {
            // Filled from the gateway cache by the handler.
            missing: Vec::new(),
        },
        notices,
        env: env(desk, settings),
        last_digest,
        character_data: None,
    }
}

/// The groups the governor runs, as `serve` configured it.
pub(super) fn groups(desk: &ConfigDesk, settings: &RuntimeSettings) -> Vec<CapacityGroup> {
    models::effective_groups(
        &settings.models,
        desk.facts.model_permits,
        &desk.facts.model_groups,
    )
}

fn env(desk: &ConfigDesk, settings: &RuntimeSettings) -> Vec<EnvRow> {
    let facts = &desk.facts;
    let set = |value: Option<&String>| value.cloned().unwrap_or_else(|| "not set".into());
    vec![
        EnvRow {
            key: "runtime.timezone",
            label: "Timezone",
            value: facts.timezone.clone(),
            reason: "Every stored time is converted with it; a change needs a restart.",
            copy: Some(facts.timezone.clone()).filter(|zone| !zone.is_empty()),
        },
        EnvRow {
            key: "runtime.boss_week_reset_weekday",
            label: "Boss week starts",
            value: format!(
                "{} {}",
                crate::api::dto::dow(settings.schedule.reset_weekday),
                crate::api::dto::hhmm(settings.schedule.reset_time)
            ),
            reason: "Defines the boss-week boundaries of every stored run.",
            // `mon`..`sun`; the reset time is runtime.boss_week_reset_time.
            copy: Some(crate::api::dto::dow(settings.schedule.reset_weekday).to_ascii_lowercase()),
        },
        EnvRow {
            key: "discord.chat_pilot_role_id",
            label: "Chat pilot role",
            value: set(facts.chat_pilot_role_id.as_ref()),
            reason: "Who may talk to the chatbot is a deployment decision.",
            copy: facts.chat_pilot_role_id.clone(),
        },
        EnvRow {
            key: "models.base_url",
            label: "Model gateway",
            value: set(facts.model_gateway.as_ref()),
            reason: "Repointing the gateway would redirect the bearer key, so only the operator changes it.",
            copy: facts.model_gateway.clone(),
        },
        if facts.model_groups.is_empty() {
            EnvRow {
                key: "models.permits",
                label: "Model permits",
                value: facts.model_permits.to_string(),
                reason: "One capacity group shared by every model role; [[models.groups]] replaces it after a restart.",
                copy: Some(facts.model_permits.to_string()),
            }
        } else {
            EnvRow {
                key: "models.groups",
                label: "Model capacity groups",
                value: groups_summary(&facts.model_groups),
                reason: "Set as [[models.groups]]; restart to apply.",
                copy: None,
            }
        },
    ]
}

/// `2 groups: local 2, cloud 4`.
fn groups_summary(groups: &[CapacityGroup]) -> String {
    let each: Vec<String> = groups
        .iter()
        .map(|group| format!("{} {}", group.name, group.permits))
        .collect();
    let noun = if groups.len() == 1 { "group" } else { "groups" };
    format!("{} {noun}: {}", groups.len(), each.join(", "))
}
