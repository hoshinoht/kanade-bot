//! Module `character_data` in serve: read the optional Nexon key (a set
//! but unreadable file refuses startup), open the separate character-data
//! file (its directory created 0700, owned under the main store's lock
//! directory), load the scouter's stored estimates (with a scouter URL; they
//! are written through to the file from then on and answer lookups for the
//! live `estimate_days`), attach the service to the admin API (with the
//! `web` feature) and the Discord side (with the `bot` component) and run
//! the joined `CharacterData` task. Off, none of this runs and no file is
//! created.
//!
//! The task sweeps at startup (after that first sweep, admin requests left
//! pending become active and fetch, and roster rows take the class, level
//! and IGN case their fresh NEXON data holds) and then every
//! [`SWEEP_EVERY`] (and whenever an unlink wakes it), runs the daily batch
//! at 04:00 Singapore time and retries characters whose retry time has
//! come. With a scouter URL it also runs the background estimate refresh
//! ([`REFRESH`]: first [`Refresh::first`] after start, so a crash loop
//! never hammers the sidecar, then every [`Refresh::every`]), which only
//! queues lookups of upcoming parties ([`upcoming`]) and never waits for
//! them.
//! Periods run on tokio's monotonic clock; the batch time and every cutoff
//! come from the injected wall clock, so a wall-clock step never stops the
//! sweeper. Stopping cancels a run in progress and joins the task before
//! either store closes; waiting background lookups are dropped.

use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::{sync::watch, task::JoinHandle};

use self::upcoming::StoreParties;
use super::store::create_private_dir;
use crate::runtime::serve::composition::Composition;
use crate::{
    app::clock::Clock,
    infrastructure::{
        character_data::{CharacterData, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        scouter::{Estimates, ScouterClient, UpcomingParties},
    },
    runtime::{
        config::{CHARACTER_DATA_DB_PATH_KEY, SCOUTER_URL_KEY, ServeConfig},
        error::Error,
    },
};
use kanade_domain::chardata::{CharDataStore, next_batch};
use kanade_store::{SqliteCharData, SqliteStore, SqliteStoreConfig, SqliteStoreError};

pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// The shortest wait before a retry pass, so a retry time that cannot move
/// (a store error) never spins.
const RETRY_FLOOR: Duration = Duration::from_secs(60);

/// When the background estimate refresh passes run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refresh {
    /// After the task starts.
    pub first: Duration,
    /// Between passes.
    pub every: Duration,
}

/// Serve's: 10 minutes after start, then hourly.
pub const REFRESH: Refresh = Refresh {
    first: Duration::from_secs(10 * 60),
    every: Duration::from_secs(60 * 60),
};

mod upcoming;

/// The running module: the service, its file and its task.
pub struct Module {
    file: Arc<SqliteCharData>,
    task: Task,
}

/// Open the module when it is on and attach it to `composition`.
pub(super) async fn start(
    config: &ServeConfig,
    store: &Arc<SqliteStore>,
    composition: &mut Composition,
    clock: Clock,
) -> Result<Option<Module>, Error> {
    start_with(
        config,
        store,
        composition,
        clock,
        NexonClient::new,
        Some(REFRESH),
    )
    .await
}

/// [`start`] building the NEXON client with `nexon` (`None`: no TLS
/// provider) and running the estimate refresh on `refresh` (`None`: off);
/// tests point NEXON at a loopback fake and turn the refresh off.
pub(super) async fn start_with(
    config: &ServeConfig,
    store: &Arc<SqliteStore>,
    composition: &mut Composition,
    clock: Clock,
    nexon: impl FnOnce(ApiKey) -> Option<NexonClient>,
    refresh: Option<Refresh>,
) -> Result<Option<Module>, Error> {
    let Some(settings) = &config.character_data else {
        return Ok(None);
    };
    let key = match &settings.nexon_key {
        Some(file) => file.read_optional()?,
        None => None,
    };
    let nexon = match key {
        Some(key) => Some(nexon(ApiKey::new(key.expose())).ok_or_else(|| {
            Error::Startup("the Nexon client could not be built (no TLS provider)".into())
        })?),
        None => None,
    };
    let scouter = match &settings.scouter_url {
        Some(url) => Some(
            ScouterClient::new(url)
                .map(|client| Estimates::new(client, url.clone(), Arc::clone(&clock)))
                .ok_or_else(|| Error::Startup(format!("{SCOUTER_URL_KEY} could not be read")))?,
        ),
        None => None,
    };
    let file = open_file(&settings.db_path, &config.store.owner_lock_dir).await?;
    let mut service = CharacterData::new(
        Arc::clone(&file) as Arc<dyn CharDataStore>,
        Arc::clone(store) as Arc<dyn RosterSide>,
        nexon,
        clock,
        Timing::default(),
    );
    // The background refresh's reads, while there are estimates to refresh.
    let mut parties = None;
    if let Some(estimates) = scouter {
        // The saved `estimate_days` and reset: the startup values, then each
        // Config save.
        let saved =
            super::context::changes_or(Some(composition.config.subscribe()), &composition.settings);
        parties = refresh.map(|refresh| {
            let port: Arc<dyn UpcomingParties> = Arc::new(StoreParties::new(
                Arc::clone(store),
                Arc::clone(&composition.catalog),
                saved.clone(),
                config.runtime.timezone,
            ));
            (port, refresh)
        });
        let estimates = estimates
            .with_store(Arc::clone(&file) as Arc<dyn CharDataStore>)
            .with_fresh_days(Arc::new(move || {
                saved.borrow().settings.character_data.estimate_days
            }));
        estimates.load().await;
        service = service.with_estimates(estimates);
    }
    if let Some(url) = privacy_url(config) {
        service = service.with_privacy_url(url);
    }
    let service = Arc::new(service);
    #[cfg(feature = "web")]
    {
        let Some(state) = Arc::get_mut(&mut composition.admin.state) else {
            drop(service);
            close(file, Duration::ZERO).await;
            return Err(Error::Startup(
                "character data could not be attached to the API".into(),
            ));
        };
        state.character_data = Some(Arc::clone(&service));
    }
    if config.components.bot {
        composition.character_data = Some(Arc::clone(&service));
    }
    kanade_log::log!(
        info,
        "character_data_started",
        connected = service.connected(),
        estimates = service.estimates().is_some(),
    );
    Ok(Some(Module {
        file,
        task: Task::start(service, SWEEP_EVERY, parties),
    }))
}

impl Module {
    /// Join the task, then close the file once the API has let go of it
    /// (waiting at most `wait`).
    pub async fn stop(self, wait: Duration) {
        self.task.stop().await;
        close(self.file, wait).await;
    }
}

/// The public `/privacy` page (the Discord notice and the admin strip link
/// it); `None` without a public listener.
pub(super) fn privacy_url(config: &ServeConfig) -> Option<String> {
    config
        .runtime
        .public_bind
        .and(config.runtime.http.public_host.as_deref())
        .map(|host| format!("https://{host}{}", crate::cards::chardata::PRIVACY_PATH))
}

/// Create the file's directory 0700 when absent (Compose: `/chardata/db`
/// inside the volume, whose root Docker may leave 0755), then open it.
async fn open_file(db_path: &Path, owner_lock_dir: &Path) -> Result<Arc<SqliteCharData>, Error> {
    if let Some(parent) = db_path.parent() {
        create_private_dir(parent, CHARACTER_DATA_DB_PATH_KEY)?;
    }
    SqliteCharData::open(&SqliteStoreConfig {
        db_path: db_path.to_path_buf(),
        owner_lock_dir: owner_lock_dir.to_path_buf(),
    })
    .await
    .map(Arc::new)
    .map_err(open_error)
}

/// File errors can carry paths and SQLite text; name the variable instead.
fn open_error(error: SqliteStoreError) -> Error {
    Error::Startup(match error {
        SqliteStoreError::Owned { .. } => {
            format!("{CHARACTER_DATA_DB_PATH_KEY} is already owned by another kanade process")
        }
        SqliteStoreError::UnsafePath { reason, .. } => {
            format!("{CHARACTER_DATA_DB_PATH_KEY}: refusing an unsafe path ({reason})")
        }
        error @ (SqliteStoreError::ChecksumMismatch { .. }
        | SqliteStoreError::FutureVersion { .. }
        | SqliteStoreError::MigrationGap { .. }) => {
            format!("character data schema refused: {error}")
        }
        _ => format!("{CHARACTER_DATA_DB_PATH_KEY}: the file could not be opened"),
    })
}

async fn close(mut file: Arc<SqliteCharData>, wait: Duration) {
    let deadline = Instant::now() + wait;
    loop {
        match Arc::try_unwrap(file) {
            Ok(owned) => {
                let closed = owned.close().await.is_ok();
                kanade_log::log!(
                    if closed { "INFO" } else { "WARN" },
                    "character_data_closed",
                    closed,
                );
                return;
            }
            Err(shared) if Instant::now() < deadline => {
                file = shared;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(_) => {
                kanade_log::log!(warn, "character_data_closed", closed = false);
                return;
            }
        }
    }
}

/// The joined `CharacterData` task; stopping it also cancels and joins a
/// running estimate lookup and drops the waiting background ones.
pub(super) struct Task {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
    service: Arc<CharacterData>,
}

/// The monotonic wait until wall-clock `at`, seen from `now`.
fn until(now: DateTime<Utc>, at: DateTime<Utc>) -> Duration {
    (at - now).to_std().unwrap_or(Duration::ZERO)
}

impl Task {
    /// `parties`: the estimate refresh's reads and when its passes run
    /// (`None`: no refresh).
    pub(super) fn start(
        service: Arc<CharacterData>,
        sweep_every: Duration,
        parties: Option<(Arc<dyn UpcomingParties>, Refresh)>,
    ) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let joined = Arc::clone(&service);
        let task = tokio::spawn(async move {
            let mut next_sweep = tokio::time::Instant::now();
            let mut next_refresh = parties
                .as_ref()
                .map(|(_, refresh)| tokio::time::Instant::now() + refresh.first);
            // After the startup sweep (which drops lapsed requests), once:
            // admin requests left pending become active and fetch, then
            // roster rows take what fresh snapshots already hold.
            let mut started = false;
            // The batch still owed, kept across wakes: whichever timer wakes
            // the loop, a wall clock at or past it runs the batch once. It
            // only moves earlier (a wall-clock step back) or, after the
            // batch, to the next 04:00 SGT; never run before it.
            let mut batch_at = next_batch(service.now());
            loop {
                let now = service.now();
                let due = now >= batch_at;
                if !due {
                    batch_at = batch_at.min(next_batch(now));
                }
                // Positive while not due, so a stepped-back clock waits
                // rather than spinning.
                let batch_in = until(now, batch_at);
                let retry_in = service
                    .next_retry()
                    .await
                    .map(|at| until(now, at).max(RETRY_FLOOR));
                // A share card whose strip failed: 5 minutes, then hourly.
                let strip_in = service
                    .next_strip_retry()
                    .map(|at| until(now, at).max(RETRY_FLOOR));
                let run = async {
                    if due {
                        service.refresh_due().await;
                        return true;
                    }
                    tokio::select! {
                        () = tokio::time::sleep_until(next_sweep) => {
                            next_sweep = tokio::time::Instant::now() + sweep_every;
                            service.sweep().await;
                            if !started {
                                service.activate_requests().await;
                                service.sync_rosters().await;
                                started = true;
                            }
                        }
                        () = service.woken() => {
                            service.sweep().await;
                        }
                        // Only wakes the loop, which checks the wall clock.
                        () = tokio::time::sleep(batch_in) => {}
                        () = tokio::time::sleep(retry_in.unwrap_or(Duration::MAX)), if retry_in.is_some() => {
                            service.retry_due().await;
                        }
                        () = tokio::time::sleep(strip_in.unwrap_or(Duration::MAX)), if strip_in.is_some() => {
                            service.strip_posts().await;
                        }
                        // Queues lookups only: the worker does them.
                        () = tokio::time::sleep_until(next_refresh.unwrap_or(next_sweep)), if next_refresh.is_some() => {
                            if let Some((port, refresh)) = &parties {
                                next_refresh = Some(tokio::time::Instant::now() + refresh.every);
                                if let Some(estimates) = service.estimates() {
                                    estimates.refresh(port.as_ref()).await;
                                }
                            }
                        }
                    }
                    false
                };
                let batched = tokio::select! {
                    batched = run => batched,
                    _ = stopped.changed() => return,
                };
                if batched {
                    batch_at = next_batch(service.now());
                }
            }
        });
        Self {
            stop,
            task,
            service: joined,
        }
    }

    pub(super) async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
        self.service.stop_estimates().await;
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
