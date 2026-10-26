//! The live settings desk the config routes, the bot's commands and serve
//! share: the settings port, the running settings (one lock serialises
//! saves), the model catalog, the persona files, plain deployment facts, and
//! the [`SettingsChanged`] channel later wiring (gateway watch lists, tick,
//! extractor, chat) subscribes to.

use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};

use chrono::{DateTime, Utc};

use serde_json::{Value, json};
use tokio::sync::{MutexGuard, watch};

use crate::chat::persona::{PersonaOption, PersonaSnapshot, PersonaStore, ProfileId};
use kanade_domain::{
    scheduler::StoreError,
    settings::{
        Models, RuntimeSettings, Section, SettingsChange, SettingsError, SettingsStore, keys,
        save_section_recorded, stored_rows,
    },
};
use kanade_llm::{
    governor::Role,
    setup::{CapacityGroup, CatalogSnapshot, RoleSwap, RunningRole},
};
use kanade_store::replays::{ReplayScope, ReplayStore, StoredReplay};

pub type ConfigFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe section writes over any [`SettingsStore`]; `change` (the
/// History record of an effective save) and `replay` (a keyed PATCH's
/// answer) are written in the same transaction as the rows.
pub trait SettingsPort: Send + Sync {
    fn save(
        &self,
        section: Section,
        change: Option<SettingsChange>,
        replay: Option<StoredReplay>,
    ) -> ConfigFuture<'_, Result<(), SettingsError>>;

    /// The env-seeded keys (the id lists and the digest channel) that have
    /// explicit save); the others follow their env seed.
    fn saved_keys(&self) -> ConfigFuture<'_, Result<Vec<&'static str>, SettingsError>>;

    /// The live Config PATCH replay of `actor`'s `key` at `now`.
    fn replay(
        &self,
        actor: String,
        key: String,
        now: DateTime<Utc>,
    ) -> ConfigFuture<'_, Result<Option<StoredReplay>, StoreError>>;

    /// Record (or update) a keyed PATCH's answer without saving rows.
    fn put_replay(&self, replay: StoredReplay) -> ConfigFuture<'_, Result<(), StoreError>>;
}

impl<T: SettingsStore + ReplayStore + Send + Sync> SettingsPort for T {
    fn save(
        &self,
        section: Section,
        change: Option<SettingsChange>,
        replay: Option<StoredReplay>,
    ) -> ConfigFuture<'_, Result<(), SettingsError>> {
        Box::pin(async move {
            match replay {
                None => save_section_recorded(self, &section, change).await,
                Some(replay) => Ok(self
                    .put_settings_rows_replayed(stored_rows(&section)?, change, replay)
                    .await?),
            }
        })
    }

    fn saved_keys(&self) -> ConfigFuture<'_, Result<Vec<&'static str>, SettingsError>> {
        Box::pin(async move {
            let rows = self.settings_rows().await?;
            Ok(SEEDED_KEYS
                .into_iter()
                .filter(|key| rows.contains_key(*key))
                .collect())
        })
    }

    fn replay(
        &self,
        actor: String,
        key: String,
        now: DateTime<Utc>,
    ) -> ConfigFuture<'_, Result<Option<StoredReplay>, StoreError>> {
        Box::pin(
            async move { ReplayStore::replay(self, ReplayScope::Config, &actor, &key, now).await },
        )
    }

    fn put_replay(&self, replay: StoredReplay) -> ConfigFuture<'_, Result<(), StoreError>> {
        Box::pin(ReplayStore::put_replay(self, replay))
    }
}

/// Keys whose value follows an env seed until a save stores a row: the id
/// lists and the digest channel. Their `*_source` is `saved` or `env`.
const SEEDED_KEYS: [&str; 4] = [
    keys::WATCHED_CHANNELS,
    keys::WATCHED_CATEGORIES,
    keys::CHAT_CATEGORIES,
    keys::POST_CHANNEL,
];

/// One catalog read: `reachable` is false when this listing failed; the
/// snapshot is then the last good one plus the declared models (only those
/// before any listing). `no_listing`: the gateway answered that it serves no
/// model list, which counts as reachable (declared models are saveable).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogRead {
    pub reachable: bool,
    pub no_listing: bool,
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
                    voice: profile.value.voice.clone().unwrap_or_default(),
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

pub struct ConfigDesk {
    pub store: Arc<dyn SettingsPort>,
    current: tokio::sync::Mutex<RuntimeSettings>,
    pub models: Option<Arc<dyn ModelCatalog>>,
    pub facts: ConfigFacts,
    pub personas: Option<PersonaFiles>,
    changes: watch::Sender<SettingsChanged>,
    /// Which seeded keys have a stored row: read from the store once, then
    /// kept by the saves (all settings writes go through this desk).
    saved: Mutex<SavedKeys>,
    /// The last switch of the running roles failed: saved and running
    /// models differ until a retry ([`Self::apply_models`]) succeeds.
    models_unapplied: AtomicBool,
}

/// The cached saved-key set and a save count, so a store read that began
/// before a save never installs its older answer.
#[derive(Default)]
struct SavedKeys {
    keys: Option<Vec<&'static str>>,
    saves: u64,
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
            saved: Mutex::new(SavedKeys::default()),
            models_unapplied: AtomicBool::new(false),
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

    /// When runs end, read live from the saved `v5.run_lengths`.
    pub fn run_ends(
        &self,
        catalog: Arc<kanade_domain::catalog::BossTable>,
        policy: kanade_domain::schedule::SchedulePolicy,
    ) -> kanade_domain::completion::RunEndsSource {
        let changes = self.subscribe();
        kanade_domain::completion::RunEndsSource::new(
            Arc::new(move || changes.borrow().settings.run_lengths.clone()),
            Some(catalog),
            policy,
        )
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

    pub async fn lock(&self) -> MutexGuard<'_, RuntimeSettings> {
        self.current.lock().await
    }

    /// The revision the next [`Self::publish`] assigns; stable while the
    /// settings lock is held.
    pub fn next_revision(&self) -> u64 {
        self.changes.borrow().revision + 1
    }

    pub fn publish(&self, section: &'static str, actor: String, settings: &RuntimeSettings) -> u64 {
        let revision = self.changes.borrow().revision + 1;
        self.changes.send_replace(SettingsChanged {
            revision,
            section: Some(section),
            actor: Some(actor),
            settings: Arc::new(settings.clone()),
        });
        revision
    }

    /// The seeded keys with a stored row. A failed first read never fails
    /// the page: it is logged, every key shows `env` for this answer, and the
    /// next read retries.
    pub async fn saved_keys(&self) -> Vec<&'static str> {
        let saves = {
            let cached = self.saved.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(keys) = &cached.keys {
                return keys.clone();
            }
            cached.saves
        };
        match self.store.saved_keys().await {
            Ok(saved) => {
                let mut cached = self.saved.lock().unwrap_or_else(PoisonError::into_inner);
                // A save landed during the read: the answer may predate it.
                if cached.saves == saves {
                    cached.keys = Some(saved.clone());
                }
                saved
            }
            // Store error text may carry paths, so only the event is logged.
            Err(_) => {
                kanade_log::log!(warn, "config_saved_lists_unreadable");
                Vec::new()
            }
        }
    }

    /// A committed save of a seeded key: it now has a row.
    pub fn note_saved(&self, key: &'static str) {
        let mut cached = self.saved.lock().unwrap_or_else(PoisonError::into_inner);
        cached.saves += 1;
        if let Some(keys) = cached.keys.as_mut()
            && !keys.contains(&key)
        {
            keys.push(key);
        }
    }

    /// A model gateway is configured (`KANADE_MODEL_BASE_URL`).
    pub fn gateway_configured(&self) -> bool {
        self.models.is_some()
    }

    pub async fn catalog(&self) -> CatalogRead {
        match &self.models {
            Some(models) => models.read().await,
            None => CatalogRead::default(),
        }
    }

    /// Switches the running roles to saved `models`; `true` when they now
    /// run (never without a gateway). A failure is logged and remembered
    /// until a later switch succeeds. Callers hold the settings lock, so a
    /// switch never installs models older than the saved ones.
    pub fn apply_models(&self, models: &Models) -> bool {
        let Some(stack) = &self.models else {
            return false;
        };
        let applied = match stack.apply(models) {
            Ok(swaps) => {
                models_applied(&swaps);
                true
            }
            Err(error) => {
                kanade_log::log!(warn, "model_roles_not_applied", error);
                false
            }
        };
        self.models_unapplied.store(!applied, Ordering::SeqCst);
        applied
    }

    /// The last switch failed: the running models are not the saved ones.
    pub fn models_unapplied(&self) -> bool {
        self.models_unapplied.load(Ordering::SeqCst)
    }

    /// `kanade.toml` keys the chatbot needs before it can be turned on.
    pub fn missing_env(&self) -> Vec<String> {
        let mut missing = Vec::new();
        if self.facts.chat_pilot_role_id.is_none() {
            missing.push("discord.chat_pilot_role_id".to_owned());
        }
        if self.facts.model_gateway.is_none() {
            missing.push("models.base_url".to_owned());
        }
        missing
    }

    pub fn running(&self) -> BTreeMap<Role, RunningRole> {
        self.models
            .as_ref()
            .map(|models| models.running())
            .unwrap_or_default()
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
fn models_applied(swaps: &[RoleSwap]) {
    for swap in swaps {
        kanade_log::log!(
            info,
            "model_role_switched",
            role = swap.role.as_str(),
            from = running(swap.before.as_ref()),
            to = running(swap.after.as_ref()),
        );
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::Notify;

    use super::*;

    /// A store whose key read waits for `gate`, to interleave a save.
    #[derive(Default)]
    struct Gated {
        keys: Mutex<Vec<&'static str>>,
        entered: Notify,
        gate: Notify,
    }

    impl SettingsPort for Gated {
        fn save(
            &self,
            _: Section,
            _: Option<SettingsChange>,
            _: Option<StoredReplay>,
        ) -> ConfigFuture<'_, Result<(), SettingsError>> {
            Box::pin(async { Ok(()) })
        }

        fn saved_keys(&self) -> ConfigFuture<'_, Result<Vec<&'static str>, SettingsError>> {
            Box::pin(async move {
                let keys = self.keys.lock().unwrap().clone();
                self.entered.notify_one();
                self.gate.notified().await;
                Ok(keys)
            })
        }

        fn replay(
            &self,
            _: String,
            _: String,
            _: DateTime<Utc>,
        ) -> ConfigFuture<'_, Result<Option<StoredReplay>, StoreError>> {
            Box::pin(async { Ok(None) })
        }

        fn put_replay(&self, _: StoredReplay) -> ConfigFuture<'_, Result<(), StoreError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[tokio::test]
    async fn a_list_read_that_began_before_a_save_is_never_cached() {
        let store = Arc::new(Gated::default());
        let desk = ConfigDesk::new(ConfigInputs {
            settings: RuntimeSettings::default(),
            store: store.clone(),
            models: None,
            facts: ConfigFacts {
                timezone: "UTC".into(),
                model_gateway: None,
                model_permits: 1,
                model_groups: Vec::new(),
                chat_pilot_role_id: None,
            },
            personas: None,
        });
        let save = async {
            store.entered.notified().await;
            store.keys.lock().unwrap().push(keys::CHAT_CATEGORIES);
            desk.note_saved(keys::CHAT_CATEGORIES);
            store.gate.notify_one();
        };
        let (stale, ()) = tokio::join!(desk.saved_keys(), save);
        assert!(stale.is_empty(), "the read began before the save");
        // The stale answer was not kept: the next read asks the store again.
        store.gate.notify_one();
        assert_eq!(desk.saved_keys().await, [keys::CHAT_CATEGORIES]);
        // Now cached: no store read (the gate holds no permit).
        assert_eq!(desk.saved_keys().await, [keys::CHAT_CATEGORIES]);
    }
}
