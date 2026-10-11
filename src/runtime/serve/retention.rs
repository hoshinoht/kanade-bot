//! Model-log retention (user decision 2026-09-25: 90 days): serve prunes the
//! extraction, chat and rewrite logs, processed cached messages and drained
//! notices older than [`DEFAULT_LOG_RETENTION`] at startup and then every
//! [`EVERY`], off the request path, on the store it owns. The period runs
//! on tokio's monotonic clock, so a wall-clock step cannot skip or repeat a
//! run; the cutoff comes from serve's injected wall clock. A run is bounded
//! by [`RUN_LIMIT`] (the store deletes in short batches, each its own
//! transaction, so a cut run just resumes the next day); stopping cancels a
//! run in progress and joins the task before the store closes.

use std::{sync::Arc, time::Duration};

use serde_json::json;
use tokio::{sync::watch, task::JoinHandle};

use crate::{
    api::auth::Clock,
    domain::{
        model_log::{DEFAULT_LOG_RETENTION, ModelLogStore, PruneCounts, retention_cutoff},
        scheduler::StoreError,
        time::to_iso,
    },
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

/// The running retention loop.
pub(super) struct Retention {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Retention {
    /// Prune now, then every `every`, as of `clock`.
    pub(super) fn start<S>(store: Arc<S>, clock: Clock, every: Duration) -> Self
    where
        S: ModelLogStore + Send + Sync + 'static,
    {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                let now = clock();
                let run = tokio::time::timeout(RUN_LIMIT, prune(&*store, now));
                tokio::select! {
                    outcome = run => report(outcome, now),
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
        Ok(Ok(counts)) => logging::event(
            "INFO",
            "model_logs_pruned",
            json!({
                "cutoff": cutoff,
                "extractions": counts.extractions,
                "chats": counts.chats,
                "messages": counts.messages,
                "notices": counts.notices,
                "rewrites": counts.rewrites,
            }),
        ),
        // Store error text may carry paths.
        Ok(Err(_)) => logging::event("WARN", "model_logs_prune_failed", json!({})),
        Err(_) => logging::event(
            "WARN",
            "model_logs_prune_cut",
            json!({"limit_seconds": RUN_LIMIT.as_secs()}),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use chrono::{DateTime, TimeDelta, TimeZone, Utc};

    use super::*;
    use crate::infrastructure::store::{MemoryScheduleStore, model_log_conformance::extraction};

    fn at(day: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 11, 0, 0, 0).unwrap() + TimeDelta::days(day)
    }

    async fn kept(store: &MemoryScheduleStore, id: &str) -> bool {
        store.load_extraction(id).await.unwrap().is_some()
    }

    async fn gone(store: &MemoryScheduleStore, id: &str) -> bool {
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
        let store = Arc::new(MemoryScheduleStore::new());
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
    }
}
