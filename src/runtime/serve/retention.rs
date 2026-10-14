//! Model-log retention (user decision 2026-09-25: 90 days): serve prunes the
//! extraction, chat and rewrite logs, processed cached messages and drained
//! notices older than [`DEFAULT_LOG_RETENTION`] at startup and then every
//! [`EVERY`], off the request path, on the store it owns, then checkpoints
//! and truncates the store's WAL (user decision 2026-10-11). The period runs
//! on tokio's monotonic clock, so a wall-clock step cannot skip or repeat a
//! run; the cutoff comes from serve's injected wall clock. A run (prune and
//! checkpoint together) is bounded by [`RUN_LIMIT`] (the store deletes in
//! short batches, each its own transaction, so a cut run just resumes the
//! next day; a cut prune leaves no time to checkpoint); stopping cancels a
//! run in progress and joins the task before the store closes.

use std::{sync::Arc, time::Duration};

use tokio::{sync::watch, task::JoinHandle};

use crate::{
    api::auth::Clock,
    domain::{
        model_log::{DEFAULT_LOG_RETENTION, ModelLogStore, PruneCounts, retention_cutoff},
        scheduler::StoreError,
        time::to_iso,
    },
    infrastructure::store::maintenance::{StoreMaintenance, WalCheckpoint},
    runtime::logging,
};

pub(super) const EVERY: Duration = Duration::from_secs(24 * 60 * 60);
pub(super) const RUN_LIMIT: Duration = Duration::from_secs(60);

/// Prune what is older than the retention as of `now`.
async fn prune<S: ModelLogStore>(
    store: &S,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<PruneCounts, StoreError> {
    match retention_cutoff(now, DEFAULT_LOG_RETENTION) {
        Some(cutoff) => store.prune_model_logs(cutoff).await,
        None => Ok(PruneCounts::default()),
    }
}

/// One run: prune, then (unless the prune used up the limit) checkpoint,
/// both within [`RUN_LIMIT`] of the start.
async fn run<S: ModelLogStore + StoreMaintenance>(store: &S, now: chrono::DateTime<chrono::Utc>) {
    let deadline = tokio::time::Instant::now() + RUN_LIMIT;
    let pruned = tokio::time::timeout_at(deadline, prune(store, now)).await;
    let cut = pruned.is_err();
    report(pruned, now);
    if !cut {
        report_checkpoint(tokio::time::timeout_at(deadline, store.checkpoint_wal()).await);
    }
}

/// The running retention loop.
pub(super) struct Retention {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Retention {
    /// Prune and checkpoint now, then every `every`, as of `clock`.
    pub(super) fn start<S>(store: Arc<S>, clock: Clock, every: Duration) -> Self
    where
        S: ModelLogStore + StoreMaintenance + Send + Sync + 'static,
    {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = run(&*store, clock()) => {}
                    _ = stopped.changed() => return,
                }
                tokio::select! {
                    () = tokio::time::sleep(every) => {}
                    _ = stopped.changed() => return,
                }
            }
        });
        Self { stop, task }
    }

    /// Stop the loop, cancelling a run in progress (its finished batches
    /// stay), and wait until the task has let go of the store.
    pub(super) async fn stop(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

fn report(
    outcome: Result<Result<PruneCounts, StoreError>, tokio::time::error::Elapsed>,
    now: chrono::DateTime<chrono::Utc>,
) {
    let cutoff =
        retention_cutoff(now, DEFAULT_LOG_RETENTION).and_then(|cutoff| to_iso(&cutoff).ok());
    match outcome {
        Ok(Ok(counts)) => logging::log!(
            info,
            "model_logs_pruned",
            cutoff,
            extractions = counts.extractions,
            chats = counts.chats,
            messages = counts.messages,
            notices = counts.notices,
            rewrites = counts.rewrites,
        ),
        // Store error text may carry paths.
        Ok(Err(_)) => logging::log!(warn, "model_logs_prune_failed"),
        Err(_) => logging::log!(
            warn,
            "model_logs_prune_cut",
            limit_seconds = RUN_LIMIT.as_secs(),
        ),
    }
}

fn report_checkpoint(
    outcome: Result<Result<WalCheckpoint, StoreError>, tokio::time::error::Elapsed>,
) {
    match outcome {
        Ok(Ok(done)) => logging::log!(
            info,
            "wal_checkpointed",
            busy = done.busy,
            log = done.log,
            checkpointed = done.checkpointed,
        ),
        // Store error text may carry paths.
        Ok(Err(_)) => logging::log!(warn, "wal_checkpoint_failed"),
        Err(_) => logging::log!(
            warn,
            "wal_checkpoint_cut",
            limit_seconds = RUN_LIMIT.as_secs(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use chrono::{DateTime, TimeDelta, TimeZone, Utc};

    use super::*;
    use crate::infrastructure::store::{SqliteStore, model_log_conformance::extraction};

    fn at(day: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 11, 0, 0, 0).unwrap() + TimeDelta::days(day)
    }

    async fn kept(store: &SqliteStore, id: &str) -> bool {
        store.load_extraction(id).await.unwrap().is_some()
    }

    async fn gone(store: &SqliteStore, id: &str) -> bool {
        for _ in 0..500 {
            if !kept(store, id).await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        false
    }

    #[tokio::test]
    async fn prunes_at_start_and_each_period_by_the_injected_clock_until_stopped() {
        logging::capture();
        let store = Arc::new(SqliteStore::open_temp().await);
        for (id, day) in [("x-old", -91), ("x-edge", -89), ("x-new", 0)] {
            store
                .record_extraction(extraction(id, at(day)))
                .await
                .unwrap();
        }
        let day = Arc::new(AtomicI64::new(0));
        let clock: Clock = {
            let day = day.clone();
            Arc::new(move || at(day.load(Ordering::SeqCst)))
        };
        let running = Retention::start(store.clone(), clock, Duration::from_millis(20));
        assert!(gone(&store, "x-old").await, "pruned at startup");
        assert!(kept(&store, "x-edge").await, "inside the 90 days");
        day.store(2, Ordering::SeqCst);
        assert!(gone(&store, "x-edge").await, "pruned on a later period");
        assert!(kept(&store, "x-new").await);
        running.stop().await;

        // Stopped: nothing prunes any more.
        store
            .record_extraction(extraction("x-late", at(-200)))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(kept(&store, "x-late").await, "the loop ended with stop");

        let pruned: Vec<_> = logging::captured()
            .into_iter()
            .filter(|line| line["event"] == "model_logs_pruned")
            .collect();
        assert!(pruned.len() >= 2, "{pruned:?}");
        assert_eq!(pruned[0]["level"], "INFO");
        assert_eq!(pruned[0]["extractions"], 1);
        assert_eq!(pruned[0]["cutoff"], to_iso(&at(-90)).unwrap());

        // Each run checkpoints right after its prune.
        let order: Vec<_> = logging::captured()
            .into_iter()
            .filter_map(|line| line["event"].as_str().map(str::to_owned))
            .filter(|event| event == "model_logs_pruned" || event == "wal_checkpointed")
            .collect();
        assert!(order.len() >= 4, "{order:?}");
        for pair in order.chunks(2) {
            assert_eq!(pair, ["model_logs_pruned", "wal_checkpointed"], "{order:?}");
        }
    }

    /// The run's checkpoint empties the WAL the writes filled, and logs its
    /// frame counts.
    #[tokio::test]
    async fn a_run_truncates_the_sqlite_wal_after_pruning() {
        logging::capture();
        let store = Arc::new(SqliteStore::open_temp().await);
        for (n, day) in [-120, -100, 0].into_iter().enumerate() {
            store
                .record_extraction(extraction(&format!("x-{n}"), at(day)))
                .await
                .unwrap();
        }
        let mut wal = store.temp_dir().config().db_path.into_os_string();
        wal.push("-wal");
        assert!(std::fs::metadata(&wal).unwrap().len() > 0);

        let running = Retention::start(store.clone(), Arc::new(|| at(0)), EVERY);
        let mut lines = Vec::new();
        for _ in 0..500 {
            lines = logging::captured();
            if lines.iter().any(|line| line["event"] == "wal_checkpointed") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        running.stop().await;
        let events: Vec<_> = lines
            .iter()
            .filter(|line| {
                line["event"] == "model_logs_pruned" || line["event"] == "wal_checkpointed"
            })
            .collect();
        assert_eq!(events.len(), 2, "{lines:?}");
        assert_eq!(events[0]["event"], "model_logs_pruned");
        assert_eq!(events[0]["extractions"], 2);
        let checkpointed = events[1];
        assert_eq!(checkpointed["level"], "INFO");
        assert_eq!(checkpointed["busy"], false);
        assert!(checkpointed["log"].is_i64() && checkpointed["checkpointed"].is_i64());
        assert_eq!(std::fs::metadata(&wal).unwrap().len(), 0, "{checkpointed}");
        assert!(store.load_extraction("x-2").await.unwrap().is_some());

        let store = Arc::into_inner(store).expect("the task let go of the store");
        store.close().await.expect("close");
    }
}
