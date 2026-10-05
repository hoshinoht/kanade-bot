//! The rescan port the admin API submits through. Submitting only queues a
//! job; the runner reads channels in its own task, so no request waits on a
//! scan. [`RescanService`] adapts the extractor's [`Rescans`] queue; the
//! API's `Idempotency-Key` memory lives in [`RescanDesk`].

use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use crate::{
    domain::{
        model_log::{ModelLogStore, RescanJob},
        scheduler::ScheduleStore,
    },
    extract::{
        pipeline::{Outbox, Proposer},
        rescan::{History, JobView, RescanError, RescanRequest, Rescans},
    },
    infrastructure::llm::LlmProvider,
};

pub type RescanFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, RescanError>> + Send + 'a>>;

/// A job as the API shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RescanView {
    pub job: RescanJob,
    /// The channel being read while the job runs.
    pub current: Option<String>,
    /// Per channel, the gated messages its read was expected to find when
    /// the job started; channels without a count are missing.
    pub expected: HashMap<String, usize>,
    /// A cancel reached the running job; it ends `cancelled` after the call
    /// in flight.
    pub stopping: bool,
}

pub trait RescanRunner: Send + Sync {
    /// Why a submit would be refused before anything is queued (extraction
    /// switched off), so a client can say so before anyone asks.
    fn ready(&self) -> Result<(), RescanError> {
        Ok(())
    }
    /// Queue a job (or hand back the one already covering these channels).
    fn submit(&self, request: RescanRequest) -> RescanFuture<'_, RescanView>;
    fn job(&self, id: String) -> RescanFuture<'_, Option<RescanView>>;
    /// Stop a job; `None` when it is unknown. A finished job is returned as is.
    fn cancel(&self, id: String) -> RescanFuture<'_, Option<RescanView>>;
}

/// [`RescanRunner`] over the extractor's queue; `Rescans::run` must be
/// spawned beside it.
pub struct RescanService<S, P, X, O, H> {
    rescans: Arc<Rescans<S, P, X, O, H>>,
    stopping: Mutex<HashSet<String>>,
}

impl<S, P, X, O, H> RescanService<S, P, X, O, H> {
    pub fn new(rescans: Arc<Rescans<S, P, X, O, H>>) -> Self {
        Self {
            rescans,
            stopping: Mutex::new(HashSet::new()),
        }
    }

    fn stopping(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.stopping
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn view(&self, view: JobView) -> RescanView {
        let JobView {
            job,
            current,
            expected,
        } = view;
        let mut stopping = self.stopping();
        let flagged = if job.status.is_final() {
            stopping.remove(&job.id);
            false
        } else {
            stopping.contains(&job.id)
        };
        RescanView {
            job,
            current,
            expected,
            stopping: flagged,
        }
    }
}

impl<S, P, X, O, H> RescanRunner for RescanService<S, P, X, O, H>
where
    S: ScheduleStore + ModelLogStore + Send + Sync + 'static,
    P: LlmProvider + 'static,
    X: Proposer + 'static,
    O: Outbox + 'static,
    H: History + 'static,
    Rescans<S, P, X, O, H>: Send + Sync,
{
    fn submit(&self, request: RescanRequest) -> RescanFuture<'_, RescanView> {
        Box::pin(async move {
            let view = self.rescans.submit(request).await?;
            Ok(self.view(view))
        })
    }

    fn job(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        Box::pin(async move { Ok(self.rescans.get(&id).await?.map(|view| self.view(view))) })
    }

    fn cancel(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        Box::pin(async move {
            let stopped = self.rescans.cancel(&id).await?;
            let Some(view) = self.rescans.get(&id).await? else {
                return Ok(None);
            };
            if stopped && !view.job.status.is_final() {
                self.stopping().insert(id);
            }
            Ok(Some(self.view(view)))
        })
    }
}

/// Idempotency keys answered from memory: `(actor, key)` → what the request
/// was and the job it named. Jobs do not outlive the process either (a
/// restart cancels queued jobs and stops the running one), so neither do keys.
pub const REMEMBERED_KEYS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remembered {
    pub actor: String,
    pub key: String,
    pub digest: String,
    pub job_id: String,
}

/// The runner plus the API's request-id memory for its writes.
pub struct RescanDesk {
    pub runner: Arc<dyn RescanRunner>,
    /// Held across a keyed submit or cancel, so a concurrent retry with the
    /// same key waits for the first and replays it.
    keys: tokio::sync::Mutex<VecDeque<Remembered>>,
}

impl RescanDesk {
    pub fn new(runner: Arc<dyn RescanRunner>) -> Self {
        Self {
            runner,
            keys: tokio::sync::Mutex::new(VecDeque::new()),
        }
    }

    pub async fn keys(&self) -> tokio::sync::MutexGuard<'_, VecDeque<Remembered>> {
        self.keys.lock().await
    }
}

/// The request recorded for this key, if any.
pub fn recall<'a>(
    keys: &'a VecDeque<Remembered>,
    actor: &str,
    key: &str,
) -> Option<&'a Remembered> {
    keys.iter()
        .find(|entry| entry.actor == actor && entry.key == key)
}

pub fn remember(keys: &mut VecDeque<Remembered>, entry: Remembered) {
    keys.push_back(entry);
    while keys.len() > REMEMBERED_KEYS {
        keys.pop_front();
    }
}
