//! What the config routes compose: the settings port, the running settings
//! (one lock serialises saves), the model catalog, the persona files, plain
//! deployment facts, and the [`SettingsChanged`] channel later wiring
//! (gateway watch lists, tick, extractor, chat) subscribes to.

use std::{
    collections::{BTreeMap, VecDeque},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
};

use tokio::sync::{MutexGuard, watch};

use super::models;
use crate::{
    api::{
        dto::config::{self as dto, ConfigView, EnvRow, KeyLimits, ManageMessages},
        state::{ChannelEntry, PersonaOption, RoleEntry},
    },
    chat::persona::{PersonaSnapshot, PersonaStore, ProfileId},
    domain::settings::{
        Models, RuntimeSettings, Section, SettingsError, SettingsStore, save_section,
    },
    infrastructure::llm::{
        governor::Role,
        setup::{CapacityGroup, CatalogSnapshot, RoleSwap, RunningRole},
    },
};

pub type ConfigFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe section writes over any [`SettingsStore`].
pub trait SettingsPort: Send + Sync {
    fn save(&self, section: Section) -> ConfigFuture<'_, Result<(), SettingsError>>;
}

impl<T: SettingsStore + Send + Sync> SettingsPort for T {
    fn save(&self, section: Section) -> ConfigFuture<'_, Result<(), SettingsError>> {
        Box::pin(async move { save_section(self, &section).await })
    }
}

/// One catalog read: `reachable` is false when this listing failed; the
/// snapshot is then the last good one (empty before any).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogRead {
    pub reachable: bool,
    pub snapshot: CatalogSnapshot,
}

/// The gateway's live model list and the running role models (`ModelStack`
/// in production).
pub trait ModelCatalog: Send + Sync {
    fn read(&self) -> ConfigFuture<'_, CatalogRead>;

    /// Switches the running roles to saved `models`; the next session of each
    /// role uses them. Returns the roles whose alias or effort changed.
    fn apply(&self, _models: &Models) -> Result<Vec<RoleSwap>, String> {
        Ok(Vec::new())
    }

    /// Alias and effort each routed role runs with now; a role waiting for
    /// a restart ([`Self::awaiting_restart`]) is left out.
    fn running(&self) -> BTreeMap<Role, RunningRole> {
        BTreeMap::new()
    }

    /// Roles that have a model now but whose feature only starts with the
    /// bot (extraction and heading rewrites had none at startup).
    fn awaiting_restart(&self) -> Vec<Role> {
        Vec::new()
    }
}

/// Deployment facts the page shows read-only (plain inputs; no env reads here).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigFacts {
    pub timezone: String,
    /// `KANADE_MODEL_BASE_URL`; `None` disables every model feature.
    pub model_gateway: Option<String>,
    /// `KANADE_MODEL_PERMITS`: the default `gateway` group's permits.
    pub model_permits: u32,
    /// `kanade.toml` `[[models.groups]]`; non-empty replaces the default group.
    pub model_groups: Vec<CapacityGroup>,
    pub chat_pilot_role_id: Option<String>,
}

/// `config/personas/` and the live snapshot chat turns pin.
pub struct PersonaFiles {
    pub dir: PathBuf,
    pub store: Arc<PersonaStore>,
}

/// Sent after every saved change. `section` is `None` for the initial value.
#[derive(Clone, Debug)]
pub struct SettingsChanged {
    pub revision: u64,
    pub section: Option<&'static str>,
    pub actor: Option<String>,
    pub settings: Arc<RuntimeSettings>,
}

/// One live view of readable profiles and the member-selectable subset.
#[derive(Clone, Debug, Default)]
pub struct LiveProfileChoices {
    pub snapshot: Option<Arc<PersonaSnapshot>>,
    pub options: Vec<PersonaOption>,
    pub readable: std::collections::BTreeSet<ProfileId>,
    pub selectable: std::collections::BTreeSet<ProfileId>,
}

impl LiveProfileChoices {
    fn new(snapshot: Option<Arc<PersonaSnapshot>>, visibility: &[String]) -> Self {
        let Some(active) = snapshot.as_ref().and_then(|snapshot| snapshot.active()) else {
            return Self {
                snapshot,
                ..Self::default()
            };
        };
        let readable = active.profiles.readable.keys().cloned().collect();
        let mut selectable = std::collections::BTreeSet::new();
        let mut options = Vec::new();
        for key in visibility {
            let Ok(id) = ProfileId::parse(key) else {
                continue;
            };
            let Some(profile) = active.profiles.get(&id) else {
                continue;
            };
            if selectable.insert(id.clone()) {
                options.push(PersonaOption {
                    key: id.to_string(),
                    name: profile.value.label.clone(),
                });
            }
        }
        Self {
            snapshot,
            options,
            readable,
            selectable,
        }
    }
}

pub struct ConfigInputs {
    pub settings: RuntimeSettings,
    pub store: Arc<dyn SettingsPort>,
    /// `None`: no gateway configured.
    pub models: Option<Arc<dyn ModelCatalog>>,
    pub facts: ConfigFacts,
    /// `None`: persona files unavailable (tests without files).
    pub personas: Option<PersonaFiles>,
}

/// Idempotency keys answered from memory, like rescan jobs: a retry after a
/// restart re-applies the patch, which is naturally repeatable.
const REMEMBERED_KEYS: usize = 256;

#[derive(Clone, Debug)]
pub(super) struct Remembered {
    pub actor: String,
    pub key: String,
    pub digest: String,
    pub notices: Vec<String>,
}

pub struct ConfigDesk {
    pub(super) store: Arc<dyn SettingsPort>,
    current: tokio::sync::Mutex<RuntimeSettings>,
    pub(super) models: Option<Arc<dyn ModelCatalog>>,
    pub(super) facts: ConfigFacts,
    pub(super) personas: Option<PersonaFiles>,
    changes: watch::Sender<SettingsChanged>,
    keys: Mutex<VecDeque<Remembered>>,
}

impl ConfigDesk {
    pub fn new(inputs: ConfigInputs) -> Self {
        let (changes, _) = watch::channel(SettingsChanged {
            revision: 0,
            section: None,
            actor: None,
            settings: Arc::new(inputs.settings.clone()),
        });
        Self {
            store: inputs.store,
            current: tokio::sync::Mutex::new(inputs.settings),
            models: inputs.models,
            facts: inputs.facts,
            personas: inputs.personas,
            changes,
            keys: Mutex::new(VecDeque::new()),
        }
    }

    /// Every saved change, latest value first. Live now: the persona switch
    /// and profile reload (the snapshot swaps) and model roles (applied to the
    /// stack by the save itself). Everything else applies once its consumer
    /// subscribes here, else at restart.
    pub fn subscribe(&self) -> watch::Receiver<SettingsChanged> {
        self.changes.subscribe()
    }

    /// The running settings.
    pub async fn settings(&self) -> RuntimeSettings {
        self.current.lock().await.clone()
    }

    /// The sole live member-choice source. It combines the latest saved list
    /// with the current readable profile snapshot on every call.
    pub fn profile_choices(&self) -> LiveProfileChoices {
        let settings = Arc::clone(&self.changes.borrow().settings);
        self.profile_choices_for(&settings)
    }

    pub fn profile_choices_for(&self, settings: &RuntimeSettings) -> LiveProfileChoices {
        let snapshot = self.personas.as_ref().map(|files| files.store.pin());
        LiveProfileChoices::new(snapshot, &settings.persona.profile_visibility)
    }

    pub(super) async fn lock(&self) -> MutexGuard<'_, RuntimeSettings> {
        self.current.lock().await
    }

    pub(super) fn publish(
        &self,
        section: &'static str,
        actor: String,
        settings: &RuntimeSettings,
    ) -> u64 {
        let revision = self.changes.borrow().revision + 1;
        self.changes.send_replace(SettingsChanged {
            revision,
            section: Some(section),
            actor: Some(actor),
            settings: Arc::new(settings.clone()),
        });
        revision
    }

    pub(super) fn recall(&self, actor: &str, key: &str) -> Option<Remembered> {
        self.keys
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|entry| entry.actor == actor && entry.key == key)
            .cloned()
    }

    pub(super) fn remember(&self, entry: Remembered) {
        let mut keys = self.keys.lock().unwrap_or_else(PoisonError::into_inner);
        keys.push_back(entry);
        while keys.len() > REMEMBERED_KEYS {
            keys.pop_front();
        }
    }

    pub(super) async fn catalog(&self) -> CatalogRead {
        match &self.models {
            Some(models) => models.read().await,
            None => CatalogRead::default(),
        }
    }

    /// Env-only facts the chatbot needs before it can be turned on.
    pub fn missing_env(&self, settings: &RuntimeSettings) -> Vec<String> {
        let mut missing = Vec::new();
        if self.facts.chat_pilot_role_id.is_none() {
            missing.push("KANADE_CHAT_PILOT_ROLE_ID".to_owned());
        }
        if settings.chatbot.category_ids.is_empty() {
            missing.push("KANADE_CHAT_CATEGORY_IDS".to_owned());
        }
        if self.facts.model_gateway.is_none() {
            missing.push("KANADE_MODEL_BASE_URL".to_owned());
        }
        missing
    }

    pub(super) fn view(
        &self,
        settings: &RuntimeSettings,
        catalog: &CatalogRead,
        channels: &[ChannelEntry],
        roles: &[RoleEntry],
        notices: Vec<String>,
        last_digest: Option<dto::LastDigest>,
    ) -> ConfigView {
        let missing_env = self.missing_env(settings);
        let choices = self.profile_choices_for(settings);
        let persona = dto::persona(
            &settings.persona.active,
            choices.snapshot.as_deref(),
            &choices.selectable,
            &settings.persona.role_profiles,
            roles,
        );
        let groups = self.groups(settings);
        let declared = !self.facts.model_groups.is_empty();
        ConfigView {
            pings: dto::pings(settings),
            watching: dto::Watching {
                paused: settings.watching.paused,
                extract_enabled: settings.watching.extract_enabled,
            },
            chatbot: dto::Chatbot {
                enabled: settings.chatbot.enabled,
                configured: missing_env.is_empty(),
                missing_env,
                member_rate: settings.chatbot.member_rate.into(),
                guild_rate: settings.chatbot.guild_rate.into(),
            },
            notifications: dto::Notifications {
                quiet_mode: settings.notifications.quiet_mode,
            },
            self_service: dto::self_service(settings),
            persona,
            models: dto::Models {
                reachable: catalog.reachable,
                catalog: catalog
                    .snapshot
                    .models
                    .iter()
                    .map(|model| dto::model_info(model, &catalog.snapshot))
                    .collect(),
                roles: dto::roles(settings, &catalog.snapshot, &self.running()),
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
                missing: Vec::new(),
            },
            notices,
            env: self.env(settings, channels),
            last_digest,
        }
    }

    pub(super) fn running(&self) -> BTreeMap<Role, RunningRole> {
        self.models
            .as_ref()
            .map(|models| models.running())
            .unwrap_or_default()
    }

    /// The groups the governor runs, as `serve` configured it.
    pub(super) fn groups(&self, settings: &RuntimeSettings) -> Vec<CapacityGroup> {
        models::effective_groups(
            &settings.models,
            self.facts.model_permits,
            &self.facts.model_groups,
        )
    }

    fn env(&self, settings: &RuntimeSettings, channels: &[ChannelEntry]) -> Vec<EnvRow> {
        let facts = &self.facts;
        let set = |value: Option<&String>| value.cloned().unwrap_or_else(|| "not set".into());
        let named = |ids: &[String]| {
            if ids.is_empty() {
                return "none".to_owned();
            }
            ids.iter()
                .map(|id| {
                    channels
                        .iter()
                        .find(|channel| &channel.id == id)
                        .map_or_else(|| id.clone(), |channel| channel.name.clone())
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        // The env form of an id list (`guild::list`); unset when empty.
        let ids = |ids: &[String]| (!ids.is_empty()).then(|| ids.join(","));
        let posting: Vec<String> = settings.posting.channel_id.iter().cloned().collect();
        vec![
            EnvRow {
                key: "KANADE_TIMEZONE",
                label: "Timezone",
                value: facts.timezone.clone(),
                reason: "Every stored time is converted with it; a change needs a restart.",
                copy: Some(facts.timezone.clone()).filter(|zone| !zone.is_empty()),
            },
            EnvRow {
                key: "KANADE_BOSS_WEEK_RESET_WEEKDAY",
                label: "Boss week starts",
                value: format!(
                    "{} {}",
                    crate::api::dto::dow(settings.schedule.reset_weekday),
                    crate::api::dto::hhmm(settings.schedule.reset_time)
                ),
                reason: "Defines the boss-week boundaries of every stored run.",
                // `mon`..`sun`; the reset time is KANADE_BOSS_WEEK_RESET_TIME.
                copy: Some(
                    crate::api::dto::dow(settings.schedule.reset_weekday).to_ascii_lowercase(),
                ),
            },
            EnvRow {
                key: "KANADE_POST_CHANNEL_ID",
                label: "Digest channel",
                value: if posting.is_empty() {
                    "not set".into()
                } else {
                    named(&posting)
                },
                reason: "Set with the guild's channel layout.",
                copy: ids(&posting),
            },
            EnvRow {
                key: "KANADE_WATCH_CHANNEL_IDS",
                label: "Watched channels",
                value: named(&settings.watching.channel_ids),
                reason: "Watching a new channel is a deliberate deploy.",
                copy: ids(&settings.watching.channel_ids),
            },
            EnvRow {
                key: "KANADE_WATCH_CATEGORY_IDS",
                label: "Watched categories",
                value: named(&settings.watching.category_ids),
                reason: "Watching a whole category is a deliberate deploy, like channels.",
                copy: ids(&settings.watching.category_ids),
            },
            EnvRow {
                key: "KANADE_CHAT_CATEGORY_IDS",
                label: "Chat categories",
                value: named(&settings.chatbot.category_ids),
                reason: "The chatbot answers in every channel of these categories; set with the channel layout.",
                copy: ids(&settings.chatbot.category_ids),
            },
            EnvRow {
                key: "KANADE_CHAT_PILOT_ROLE_ID",
                label: "Chat pilot role",
                value: set(facts.chat_pilot_role_id.as_ref()),
                reason: "Who may talk to the chatbot is a deployment decision.",
                copy: facts.chat_pilot_role_id.clone(),
            },
            EnvRow {
                key: "KANADE_MODEL_BASE_URL",
                label: "Model gateway",
                value: set(facts.model_gateway.as_ref()),
                reason: "Repointing the gateway would redirect the bearer key, so only the operator changes it.",
                copy: facts.model_gateway.clone(),
            },
            if facts.model_groups.is_empty() {
                EnvRow {
                    key: "KANADE_MODEL_PERMITS",
                    label: "Model permits",
                    value: facts.model_permits.to_string(),
                    reason: "One capacity group shared by every model role; kanade.toml [[models.groups]] replaces it after a restart.",
                    copy: Some(facts.model_permits.to_string()),
                }
            } else {
                EnvRow {
                    key: "KANADE_MODEL_PERMITS / kanade.toml [[models.groups]]",
                    label: "Model capacity groups",
                    value: groups_summary(&facts.model_groups),
                    reason: "Set in kanade.toml ([[models.groups]]); restart to apply.",
                    copy: None,
                }
            },
        ]
    }
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
