//! Module `character_data` in serve: read the optional Nexon key (a set
//! but unreadable file refuses startup), open the separate character-data
//! file (its directory created 0700, owned under the main store's lock
//! directory), attach the service to the admin API and run the joined
//! `CharacterData` task. Off, none of this runs and no file is created.
//!
//! The task sweeps at startup and then every [`SWEEP_EVERY`] (and whenever
//! an unlink wakes it), runs the daily batch at 04:00 Singapore time and
//! retries characters whose retry time has come. Periods run on tokio's
//! monotonic clock; the batch time and every cutoff come from the injected
//! wall clock, so a wall-clock step never stops the sweeper. Stopping
//! cancels a run in progress and joins the task before either store closes.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use tokio::{sync::watch, task::JoinHandle};

use super::{api::Composition, store::create_private_dir};
use crate::{
    api::auth::Clock,
    domain::chardata::{CharDataStore, next_batch},
    infrastructure::{
        character_data::{CharacterData, RosterSide, Timing},
        nexon::{ApiKey, NexonClient},
        scouter::{Estimates, ScouterClient},
        store::{SqliteCharData, SqliteStore, SqliteStoreConfig, SqliteStoreError},
    },
    runtime::{
        config::{
            CHARACTER_DATA_DB_PATH_KEY, NEXON_API_KEY_FILE_KEY, SCOUTER_URL_KEY, ServeConfig,
        },
        error::Error,
        logging,
        secrets::Redacted,
    },
};

pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// The shortest wait before a retry pass, so a retry time that cannot move
/// (a store error) never spins.
const RETRY_FLOOR: Duration = Duration::from_secs(60);

/// The running module: the service, its file and its task.
pub(super) struct Module {
    file: Arc<SqliteCharData>,
    task: Task,
}

/// Open the module when it is on and attach it to `composition`'s API.
pub(super) async fn start(
    config: &ServeConfig,
    store: &Arc<SqliteStore>,
    composition: &mut Composition,
    clock: Clock,
) -> Result<Option<Module>, Error> {
    let Some(settings) = &config.character_data else {
        return Ok(None);
    };
    let key = settings
        .nexon_key_file
        .as_deref()
        .map(|path| Redacted::read(path, NEXON_API_KEY_FILE_KEY))
        .transpose()?;
    let nexon = match key {
        Some(key) => Some(NexonClient::new(ApiKey::new(key.expose())).ok_or_else(|| {
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
    if let Some(parent) = settings.db_path.parent() {
        create_private_dir(parent, CHARACTER_DATA_DB_PATH_KEY)?;
    }
    let file = SqliteCharData::open(&SqliteStoreConfig {
        db_path: settings.db_path.clone(),
        owner_lock_dir: config.store.owner_lock_dir.clone(),
    })
    .await
    .map(Arc::new)
    .map_err(open_error)?;
    let mut service = CharacterData::new(
        Arc::clone(&file) as Arc<dyn CharDataStore>,
        Arc::clone(store) as Arc<dyn RosterSide>,
        nexon,
        clock,
        Timing::default(),
    );
    if let Some(estimates) = scouter {
        service = service.with_estimates(estimates);
    }
    let service = Arc::new(service);
    let Some(state) = Arc::get_mut(&mut composition.admin.state) else {
        drop(service);
        close(file, Duration::ZERO).await;
        return Err(Error::Startup(
            "character data could not be attached to the API".into(),
        ));
    };
    state.character_data = Some(Arc::clone(&service));
    logging::event(
        "INFO",
        "character_data_started",
        serde_json::json!({
            "connected": service.connected(),
            "estimates": service.estimates().is_some(),
        }),
    );
    Ok(Some(Module {
        file,
        task: Task::start(service, SWEEP_EVERY),
    }))
}

impl Module {
    /// Join the task, then close the file once the API has let go of it
    /// (waiting at most `wait`).
    pub(super) async fn stop(self, wait: Duration) {
        self.task.stop().await;
        close(self.file, wait).await;
    }
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
                logging::event(
                    if closed { "INFO" } else { "WARN" },
                    "character_data_closed",
                    serde_json::json!({ "closed": closed }),
                );
                return;
            }
            Err(shared) if Instant::now() < deadline => {
                file = shared;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(_) => {
                logging::event(
                    "WARN",
                    "character_data_closed",
                    serde_json::json!({ "closed": false }),
                );
                return;
            }
        }
    }
}

/// The joined `CharacterData` task; stopping it also cancels and joins a
/// running estimate lookup.
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
    pub(super) fn start(service: Arc<CharacterData>, sweep_every: Duration) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let joined = Arc::clone(&service);
        let task = tokio::spawn(async move {
            let mut next_sweep = tokio::time::Instant::now();
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
