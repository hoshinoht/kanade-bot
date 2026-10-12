//! The character-data service (module `character_data`): link requests and
//! confirmations, unlink, refreshes and the admin's unstored lookup through
//! the Open API under the shared limiter, budget and backoffs, and the
//! sweeper. One instance per process,
//! shared by the admin API, the `CharacterData` task and (S3) Discord.
//!
//! Logs carry error codes, counts and reasons only: never an IGN, a Nexon
//! field or an ocid. History rows (section `characters`, key
//! `link.<character>`) carry the status word and the actor only.

mod calls;
mod health;
mod link;
mod lookup;
mod member;
mod posts;
mod refresh;
mod sweep;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Notify;

pub use health::{BatchReport, Health, LinkCounts, NexonState, PostCounts};
pub use link::{ConfirmOutcome, LinkView, RequestOutcome};
pub use lookup::{Found, LOOKUP_SPACING, LookupOutcome};
pub use member::{MemberCharacter, MemberLink};
pub use posts::{DeskFuture, Located, PostDesk, PostIssue, StripReport, Stripped, Tracked};
pub use refresh::{FetchState, RefreshOutcome};

use crate::domain::characters::{Character, CharacterStore};
use crate::domain::chardata::{CharDataStore, StoreFuture};
use crate::domain::history::Actor;
use crate::domain::members::{MemberProfile, MemberStore};
use crate::domain::settings::{SettingsChange, SettingsChangeQuery, SettingsStore};
use crate::infrastructure::nexon::{Limiter, NexonClient, SPACING};
use crate::infrastructure::scouter::Estimates;
use crate::infrastructure::store::replays::{ReplayStore, StoredReplay};
pub use sweep::{SWEEPER, SweepReport};

/// The injected aware wall clock (cutoffs and stamps).
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// What the service reads from and writes to the main store: roster
/// characters, member rows and the History row of a link change.
pub trait RosterSide: Send + Sync {
    fn character(&self, id: String) -> StoreFuture<'_, Option<Character>>;
    fn member(&self, user_id: String) -> StoreFuture<'_, Option<MemberProfile>>;
    /// One member's roster characters, retired included, oldest first.
    fn member_characters(&self, member_id: String) -> StoreFuture<'_, Vec<Character>>;
    /// The History row (and a keyed request's replay) in one transaction.
    fn record(&self, change: SettingsChange, replay: Option<StoredReplay>) -> StoreFuture<'_, ()>;
    /// `actor`'s latest History row naming `link.<character_id>`.
    fn last_link_audit(
        &self,
        character_id: String,
        actor: Actor,
    ) -> StoreFuture<'_, Option<SettingsChange>>;
}

impl<T> RosterSide for T
where
    T: CharacterStore + MemberStore + SettingsStore + ReplayStore + Send + Sync,
{
    fn character(&self, id: String) -> StoreFuture<'_, Option<Character>> {
        Box::pin(async move { CharacterStore::character(self, &id).await })
    }

    fn member_characters(&self, member_id: String) -> StoreFuture<'_, Vec<Character>> {
        Box::pin(async move { CharacterStore::member_characters(self, &member_id).await })
    }

    fn member(&self, user_id: String) -> StoreFuture<'_, Option<MemberProfile>> {
        Box::pin(async move { self.load_member(&user_id).await })
    }

    fn record(&self, change: SettingsChange, replay: Option<StoredReplay>) -> StoreFuture<'_, ()> {
        Box::pin(async move {
            match replay {
                Some(replay) => {
                    self.put_settings_rows_replayed(Vec::new(), Some(change), replay)
                        .await
                }
                None => self
                    .put_settings_rows_recorded(Vec::new(), change)
                    .await
                    .map(drop),
            }
        })
    }

    fn last_link_audit(
        &self,
        character_id: String,
        actor: Actor,
    ) -> StoreFuture<'_, Option<SettingsChange>> {
        Box::pin(async move {
            let key = format!("link.{character_id}");
            let query = SettingsChangeQuery {
                actor: Some(actor),
                ..SettingsChangeQuery::default()
            };
            Ok(self
                .settings_changes(query)
                .await?
                .into_iter()
                .filter(|change| change.values.contains_key(&key))
                .max_by_key(|change| (change.at, change.id)))
        })
    }
}

/// Waits the service takes; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Between request starts.
    pub spacing: Duration,
    /// The 429 backoff's first step (1 s, then doubling).
    pub rate_limit_unit: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            spacing: SPACING,
            rate_limit_unit: Duration::from_secs(1),
        }
    }
}

/// Process-wide call holds and the last runs, for health.
#[derive(Default)]
struct Holds {
    /// `OPENAPI00002`/`00005`: nothing is called until a restart.
    key_refused: bool,
    maintenance_until: Option<DateTime<Utc>>,
    maintenance_strikes: u32,
    last_batch: Option<BatchReport>,
    last_sweep: Option<DateTime<Utc>>,
    /// Posts whose strip failed, by ref.
    strip_retries: HashMap<String, posts::StripRetry>,
    /// Each admin's last lookup (`kind:id`), for [`LOOKUP_SPACING`].
    lookups: HashMap<String, DateTime<Utc>>,
}

pub struct CharacterData {
    store: Arc<dyn CharDataStore>,
    roster: Arc<dyn RosterSide>,
    /// `None` without `KANADE_NEXON_API_KEY_FILE` (`no_key`).
    nexon: Option<NexonClient>,
    clock: Clock,
    timing: Timing,
    limiter: Limiter,
    holds: Mutex<Holds>,
    /// Wakes the sweeper (an unlink).
    wake: Notify,
    /// The Discord side that strips posts; unset without a gateway.
    desk: OnceLock<Arc<dyn PostDesk>>,
    /// MapleScouter estimates; `None` without `KANADE_SCOUTER_URL`.
    estimates: Option<Estimates>,
    /// The public `/privacy` page; `None` without a public listener.
    privacy_url: Option<String>,
}

impl std::fmt::Debug for CharacterData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CharacterData")
            .field("connected", &self.nexon.is_some())
            .finish_non_exhaustive()
    }
}

impl CharacterData {
    pub fn new(
        store: Arc<dyn CharDataStore>,
        roster: Arc<dyn RosterSide>,
        nexon: Option<NexonClient>,
        clock: Clock,
        timing: Timing,
    ) -> Self {
        Self {
            store,
            roster,
            nexon,
            clock,
            timing,
            limiter: Limiter::new(timing.spacing),
            holds: Mutex::default(),
            wake: Notify::new(),
            desk: OnceLock::new(),
            estimates: None,
            privacy_url: None,
        }
    }

    /// With MapleScouter estimates (`KANADE_SCOUTER_URL` set).
    #[must_use]
    pub fn with_estimates(mut self, estimates: Estimates) -> Self {
        self.estimates = Some(estimates);
        self
    }

    /// `None` while estimates are off.
    pub fn estimates(&self) -> Option<&Estimates> {
        self.estimates.as_ref()
    }

    /// With the public privacy page's URL (a public listener is set).
    #[must_use]
    pub fn with_privacy_url(mut self, url: String) -> Self {
        self.privacy_url = Some(url);
        self
    }

    /// The public `/privacy` page the admin strip links; `None` without a
    /// public listener.
    pub fn privacy_url(&self) -> Option<&str> {
        self.privacy_url.as_deref()
    }

    /// Cancel and join a running estimate lookup (shutdown, before the
    /// stores close).
    pub async fn stop_estimates(&self) {
        if let Some(estimates) = &self.estimates {
            estimates.stop().await;
        }
    }

    /// A Nexon key is configured.
    pub fn connected(&self) -> bool {
        self.nexon.is_some()
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// Resolves when an unlink asks the sweeper to run now.
    pub async fn woken(&self) {
        self.wake.notified().await;
    }

    /// The store, for the S3 post pipeline (`create_post` before a send,
    /// `posts`, `set_post_*`, `delete_post`).
    pub fn store(&self) -> &Arc<dyn CharDataStore> {
        &self.store
    }

    fn holds(&self) -> MutexGuard<'_, Holds> {
        self.holds.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One structured log line with codes and counts only.
fn log(level: &'static str, event: &'static str, fields: serde_json::Value) {
    crate::runtime::logging::event(level, event, fields);
}
