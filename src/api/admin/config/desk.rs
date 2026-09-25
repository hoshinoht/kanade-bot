//! What the config routes compose: the settings port, the running settings
//! (one lock serialises saves), the model catalog, the persona files, plain
//! deployment facts, and the [`SettingsChanged`] channel later wiring
//! (gateway watch lists, tick, extractor, chat) subscribes to.

use std::{
    collections::VecDeque,
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
        state::ChannelEntry,
    },
    chat::persona::PersonaStore,
    domain::settings::{RuntimeSettings, Section, SettingsError, SettingsStore, save_section},
    infrastructure::llm::setup::CatalogSnapshot,
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

/// The gateway's live model list (`ModelStack` in production).
pub trait ModelCatalog: Send + Sync {
    fn read(&self) -> ConfigFuture<'_, CatalogRead>;
}

/// Deployment facts the page shows read-only (plain inputs; no env reads here).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigFacts {
    pub timezone: String,
    /// `KANADE_MODEL_BASE_URL`; `None` disables every model feature.
    pub model_gateway: Option<String>,
    /// `KANADE_MODEL_PERMITS`: the one gateway group's permits.
    pub model_permits: u32,
    pub allow_external_unmasked: bool,
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
    /// and profile reload (the snapshot swaps). Everything else applies once
    /// its consumer subscribes here, else at restart.
    pub fn subscribe(&self) -> watch::Receiver<SettingsChanged> {
        self.changes.subscribe()
    }

    /// The running settings.
    pub async fn settings(&self) -> RuntimeSettings {
        self.current.lock().await.clone()
    }

    pub(super) async fn lock(&self) -> MutexGuard<'_, RuntimeSettings> {
        self.current.lock().await
    }

    pub(super) fn publish(&self, section: &'static str, actor: String, settings: &RuntimeSettings) {
        let revision = self.changes.borrow().revision + 1;
        self.changes.send_replace(SettingsChanged {
            revision,
            section: Some(section),
            actor: Some(actor),
            settings: Arc::new(settings.clone()),
        });
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
        notices: Vec<String>,
    ) -> ConfigView {
        let missing_env = self.missing_env(settings);
        let snapshot = self.personas.as_ref().map(|files| files.store.pin());
        let persona = match &snapshot {
            Some(snapshot) => dto::persona(&settings.persona.active, snapshot),
            None => dto::Persona {
                active: settings.persona.active.clone(),
                personas: Vec::new(),
                profiles: Vec::new(),
                role_profiles: Vec::new(),
            },
        };
        let permits = self.facts.model_permits;
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
                    .map(dto::model_info)
                    .collect(),
                roles: dto::roles(settings),
                // The governor has one env-sized group shared by every role
                // alias; the per-row DTO cannot state that faithfully.
                groups: Vec::new(),
                alias_limits: dto::alias_limits(&catalog.snapshot),
                // No key limit is declared yet; the governor's total bounds it.
                key_limits: KeyLimits {
                    max_in_flight: permits,
                    shared: true,
                },
                capacity_check: models::capacity(
                    &settings.models,
                    permits,
                    catalog.reachable.then_some(&catalog.snapshot),
                ),
                pii_pseudonymise: false,
            },
            manage_messages: ManageMessages {
                missing: Vec::new(),
            },
            notices,
            env: self.env(settings, channels),
        }
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
        let on = |flag: bool| if flag { "on" } else { "off" }.to_owned();
        let posting: Vec<String> = settings.posting.channel_id.iter().cloned().collect();
        vec![
            EnvRow {
                key: "KANADE_TIMEZONE",
                label: "Timezone",
                value: facts.timezone.clone(),
                reason: "Every stored time is converted with it; a change needs a restart.",
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
            },
            EnvRow {
                key: "KANADE_WATCH_CHANNEL_IDS",
                label: "Watched channels",
                value: named(&settings.watching.channel_ids),
                reason: "Watching a new channel is a deliberate deploy.",
            },
            EnvRow {
                key: "KANADE_WATCH_CATEGORY_IDS",
                label: "Watched categories",
                value: named(&settings.watching.category_ids),
                reason: "Watching a whole category is a deliberate deploy, like channels.",
            },
            EnvRow {
                key: "KANADE_CHAT_CATEGORY_IDS",
                label: "Chat categories",
                value: named(&settings.chatbot.category_ids),
                reason: "The chatbot answers in every channel of these categories; set with the channel layout.",
            },
            EnvRow {
                key: "KANADE_CHAT_PILOT_ROLE_ID",
                label: "Chat pilot role",
                value: set(facts.chat_pilot_role_id.as_ref()),
                reason: "Who may talk to the chatbot is a deployment decision.",
            },
            EnvRow {
                key: "KANADE_MODEL_BASE_URL",
                label: "Model gateway",
                value: set(facts.model_gateway.as_ref()),
                reason: "Repointing the gateway would redirect the bearer key, so only the operator changes it.",
            },
            EnvRow {
                key: "KANADE_MODEL_PERMITS",
                label: "Model permits",
                value: facts.model_permits.to_string(),
                reason: "One capacity group shared by every model role; groups are not editable here yet.",
            },
            EnvRow {
                key: "pseudonymisation",
                label: "PII pseudonymisation",
                value: "off".into(),
                reason: "Not available in this build: member names reach the model as written, so models that leave the homelab are refused.",
            },
            EnvRow {
                key: "KANADE_ALLOW_EXTERNAL_UNMASKED",
                label: "Unmasked external models",
                value: on(facts.allow_external_unmasked),
                reason: "Lets models that leave the homelab see member data unmasked (provider testing only); a privacy control only the operator may change.",
            },
        ]
    }
}
