//! The rescan queue (v4 `RescanWorker`): submit returns at once, one worker
//! runs jobs in order, each job's state is written to `rescan_jobs` at every
//! transition (queued → running → done | failed | cancelled) and after each
//! channel, so progress survives in the store.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::sync::Notify;

use super::read::{Pace, Reader};
use super::{History, RescanError, RescanRequest, resolve_window};
use crate::domain::model_log::{ModelLogStore, RescanJob, RescanStatus};
use crate::domain::scheduler::ScheduleStore;
use crate::extract::pipeline::{Extractor, Outbox, Proposer};
use crate::infrastructure::llm::LlmProvider;

/// Finished jobs kept in memory for progress polling (v4 `KEEP_JOBS`); the
/// store keeps them all.
pub const KEEP_JOBS: usize = 20;
/// Why jobs ended when extraction was switched off.
pub const SWITCHED_OFF: &str = "switched off";
/// Why a job a previous process left queued or running ended.
pub const INTERRUPTED: &str = "interrupted";
/// Enough rows to reach every job a crash could have left open.
const RECOVER_ROWS: u32 = 10_000;

/// A job and, while it runs, the channel being read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobView {
    pub job: RescanJob,
    pub current: Option<String>,
}

struct Tracked {
    job: RescanJob,
    stop: Arc<AtomicBool>,
    current: Option<String>,
    /// The cancelled job's `error` when a stop had a reason.
    reason: Option<String>,
    unprocessed_only: bool,
}

#[derive(Default)]
struct State {
    jobs: HashMap<String, Tracked>,
    order: Vec<String>,
    queue: VecDeque<String>,
    closed: bool,
}

impl State {
    fn view(&self, id: &str) -> Option<JobView> {
        self.jobs.get(id).map(|tracked| JobView {
            job: tracked.job.clone(),
            current: tracked.current.clone(),
        })
    }

    /// The newest queued or running job.
    fn active(&self) -> Option<&Tracked> {
        self.order
            .iter()
            .rev()
            .filter_map(|id| self.jobs.get(id))
            .find(|tracked| !tracked.job.status.is_final())
    }

    /// Flag a running job to stop; `false` once its final status is
    /// latched. With [`State::settle`] under the same lock, a stop that is
    /// accepted always ends the job `cancelled`.
    fn stop_running(&self, id: &str) -> bool {
        match self.jobs.get(id) {
            Some(tracked) if tracked.job.status == RescanStatus::Running => {
                tracked.stop.store(true, Ordering::SeqCst);
                true
            }
            _ => false,
        }
    }

    /// Latch a finished job's status: a stop accepted while it ran wins.
    fn settle(&mut self, job: &mut RescanJob) {
        if let Some(tracked) = self.jobs.get_mut(&job.id) {
            if tracked.stop.load(Ordering::SeqCst) {
                job.status = RescanStatus::Cancelled;
                if let Some(reason) = &tracked.reason {
                    job.error = Some(reason.clone());
                }
            }
            tracked.job = job.clone();
            tracked.current = None;
        }
    }

    fn remember(&mut self, tracked: Tracked) {
        self.order.push(tracked.job.id.clone());
        self.jobs.insert(tracked.job.id.clone(), tracked);
        // Evict finished jobs only; a live one is never forgotten.
        while self.order.len() > KEEP_JOBS {
            let Some(index) = self
                .order
                .iter()
                .position(|id| self.jobs.get(id).is_none_or(|t| t.job.status.is_final()))
            else {
                break;
            };
            let id = self.order.remove(index);
            self.jobs.remove(&id);
        }
    }
}

pub struct Rescans<S, P, X, O, H> {
    extractor: Arc<Extractor<S, P, X, O>>,
    history: Arc<H>,
    state: Mutex<State>,
    /// Serialises submit and close (they await store writes).
    admission: tokio::sync::Mutex<()>,
    wake: Notify,
}

impl<S, P, X, O, H> std::fmt::Debug for Rescans<S, P, X, O, H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rescans").finish_non_exhaustive()
    }
}

impl<S, P, X, O, H> Rescans<S, P, X, O, H>
where
    S: ScheduleStore + ModelLogStore + Send + Sync,
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
    H: History,
{
    pub fn new(extractor: Arc<Extractor<S, P, X, O>>, history: Arc<H>) -> Self {
        Self {
            extractor,
            history,
            state: Mutex::new(State::default()),
            admission: tokio::sync::Mutex::new(()),
            wake: Notify::new(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue a rescan, or hand back the job already covering these channels:
    /// a running one is attached to, a queued one with the same window is
    /// returned, and a queued one with another window is cancelled (its
    /// request fields never change in the store) for a new job.
    pub async fn submit(&self, request: RescanRequest) -> Result<JobView, RescanError> {
        let mut channels: Vec<String> = Vec::new();
        for channel in request.channels {
            if !channels.contains(&channel) {
                channels.push(channel);
            }
        }
        if channels.is_empty() {
            return Err(RescanError::NoChannels);
        }
        resolve_window(&request.window, request.automated).map_err(RescanError::Window)?;
        // Held across the store writes so the active-job check and the
        // replacement or insert are one step for concurrent submits.
        let _admission = self.admission.lock().await;
        let replaced = {
            let state = self.state();
            if state.closed {
                return Err(RescanError::Closed);
            }
            match state.active() {
                Some(active) if channels.iter().all(|c| active.job.channels.contains(c)) => {
                    let same_window = active.job.window == request.window
                        && active.job.automated == request.automated;
                    if active.job.status == RescanStatus::Running || same_window {
                        return Ok(state.view(&active.job.id).expect("tracked"));
                    }
                    Some(active.job.id.clone())
                }
                _ => None,
            }
        };
        if let Some(id) = replaced
            && !self
                .finish_queued(&id, "replaced by a newer request")
                .await?
        {
            // The worker started it meanwhile: attach, as to any running job.
            // A concurrent cancel also lands here; then queue the new request.
            if let Some(view) = self.state().view(&id)
                && view.job.status == RescanStatus::Running
            {
                return Ok(view);
            }
        }
        let job = RescanJob {
            id: self.extractor.new_id(),
            channels,
            window: request.window,
            source: request.source,
            automated: request.automated,
            requested_by: request.requested_by,
            status: RescanStatus::Queued,
            created_at: self.extractor.now(),
            started_at: None,
            finished_at: None,
            results: Value::Array(Vec::new()),
            error: None,
        };
        self.extractor
            .store()
            .insert_rescan_job(job.clone())
            .await?;
        let view = JobView {
            job: job.clone(),
            current: None,
        };
        {
            let mut state = self.state();
            state.queue.push_back(job.id.clone());
            state.remember(Tracked {
                job,
                stop: Arc::new(AtomicBool::new(false)),
                current: None,
                reason: None,
                unprocessed_only: request.unprocessed_only,
            });
        }
        self.wake.notify_one();
        Ok(view)
    }

    /// Ask a job to stop: a queued one is cancelled now, a running one
    /// between bursts. `false` when it is unknown or already finished.
    pub async fn cancel(&self, id: &str) -> Result<bool, RescanError> {
        let queued = {
            let state = self.state();
            match state.jobs.get(id) {
                Some(tracked) if tracked.job.status == RescanStatus::Queued => true,
                _ => return Ok(state.stop_running(id)),
            }
        };
        if queued && !self.finish_queued(id, "").await? {
            // Started meanwhile: stop it between bursts instead.
            return Ok(self.state().stop_running(id));
        }
        Ok(true)
    }

    /// A job's current state: in memory while tracked, else from the store.
    pub async fn get(&self, id: &str) -> Result<Option<JobView>, RescanError> {
        if let Some(view) = self.state().view(id) {
            return Ok(Some(view));
        }
        Ok(self
            .extractor
            .store()
            .load_rescan_job(id)
            .await?
            .map(|job| JobView { job, current: None }))
    }

    /// Jobs waiting to be reached (the running one excluded).
    pub fn queued(&self) -> usize {
        self.state().queue.len()
    }

    /// Stop taking work: queued jobs are cancelled, the running one stops
    /// between bursts, and [`Rescans::run`] returns once it has.
    pub async fn close(&self) {
        let _admission = self.admission.lock().await;
        let queued: Vec<String> = {
            let mut state = self.state();
            state.closed = true;
            for tracked in state.jobs.values() {
                if tracked.job.status == RescanStatus::Running {
                    tracked.stop.store(true, Ordering::SeqCst);
                }
            }
            state.queue.iter().cloned().collect()
        };
        for id in queued {
            // Best effort: a store failure leaves the row queued, as v4 left it.
            let _ = self.finish_queued(&id, "shut down").await;
        }
        self.wake.notify_one();
    }

    /// Extraction was switched off: queued jobs end now and the running one
    /// before its next burst, each `cancelled` with [`SWITCHED_OFF`]. Unlike
    /// [`Self::close`], new jobs are accepted again.
    pub async fn switched_off(&self) {
        let _admission = self.admission.lock().await;
        let queued: Vec<String> = {
            let mut state = self.state();
            for tracked in state.jobs.values_mut() {
                if tracked.job.status == RescanStatus::Running {
                    tracked.reason = Some(SWITCHED_OFF.to_owned());
                    tracked.stop.store(true, Ordering::SeqCst);
                }
            }
            state.queue.iter().cloned().collect()
        };
        for id in queued {
            let _ = self.finish_queued(&id, SWITCHED_OFF).await;
        }
    }

    /// Before the worker starts: jobs a previous process left queued or
    /// running (a crash, an abort) end `cancelled` with [`INTERRUPTED`], so
    /// neither the API nor health shows them as live. Jobs of this process
    /// are left alone.
    pub async fn recover(&self) -> Result<usize, RescanError> {
        // A submit writes its row before remembering the job; holding
        // admission keeps a just-queued job from looking abandoned.
        let _admission = self.admission.lock().await;
        let rows = self
            .extractor
            .store()
            .recent_rescan_jobs(RECOVER_ROWS)
            .await?;
        let mut ended = 0;
        for mut job in rows.into_iter().filter(|job| !job.status.is_final()) {
            if self.state().jobs.contains_key(&job.id) {
                continue;
            }
            job.status = RescanStatus::Cancelled;
            job.finished_at = Some(self.extractor.now());
            job.error = Some(INTERRUPTED.to_owned());
            if self.extractor.store().update_rescan_job(job).await? {
                ended += 1;
            }
        }
        Ok(ended)
    }

    /// `false` when the job was no longer queued (the worker took it).
    async fn finish_queued(&self, id: &str, reason: &str) -> Result<bool, RescanError> {
        let job = {
            let mut state = self.state();
            state.queue.retain(|queued| queued != id);
            let Some(tracked) = state.jobs.get_mut(id) else {
                return Ok(false);
            };
            if tracked.job.status != RescanStatus::Queued {
                return Ok(false);
            }
            tracked.job.status = RescanStatus::Cancelled;
            tracked.job.finished_at = Some(self.extractor.now());
            tracked.job.error = (!reason.is_empty()).then(|| reason.to_owned());
            tracked.job.clone()
        };
        self.extractor.store().update_rescan_job(job).await?;
        Ok(true)
    }

    /// The worker: one job at a time, in submission order, until closed.
    pub async fn run(&self) {
        loop {
            let next = {
                let mut state = self.state();
                if state.closed {
                    return;
                }
                state.queue.pop_front()
            };
            match next {
                Some(id) => self.run_job(&id).await,
                None => self.wake.notified().await,
            }
        }
    }

    async fn run_job(&self, id: &str) {
        let started = {
            let mut state = self.state();
            let Some(tracked) = state.jobs.get_mut(id) else {
                return;
            };
            if tracked.job.status != RescanStatus::Queued {
                return;
            }
            tracked.job.status = RescanStatus::Running;
            tracked.job.started_at = Some(self.extractor.now());
            (
                tracked.job.clone(),
                tracked.stop.clone(),
                tracked.unprocessed_only,
            )
        };
        let (mut job, stop, unprocessed_only) = started;
        // A job whose row cannot be written still runs; its end is retried.
        let _ = self.extractor.store().update_rescan_job(job.clone()).await;

        let window = match resolve_window(&job.window, job.automated) {
            Ok(window) => window,
            Err(error) => {
                self.finish(&mut job, RescanStatus::Failed, Some(error.to_string()))
                    .await;
                return;
            }
        };
        let mut pace = Pace::new(self.extractor.config().drain_interval);
        let mut results: Vec<Value> = Vec::new();
        let mut failure: Option<String> = None;
        for channel in job.channels.clone() {
            if !self.extractor.guild().extraction_enabled() {
                stop.store(true, Ordering::SeqCst);
            }
            if stop.load(Ordering::SeqCst) {
                break;
            }
            self.set_current(id, Some(channel.clone()));
            let mut reader = Reader {
                extractor: self.extractor.as_ref(),
                history: self.history.as_ref(),
                stop: stop.as_ref(),
                pace: &mut pace,
                unprocessed_only,
            };
            match reader.read(&channel, window, job.automated).await {
                Ok(result) => results.push(result),
                // The other channels still get read.
                Err(error) => {
                    failure.get_or_insert_with(|| format!("{channel}: {error}"));
                }
            }
            job.results = Value::Array(results.clone());
            self.set_results(id, job.results.clone());
            let _ = self.extractor.store().update_rescan_job(job.clone()).await;
        }
        // The reader stops itself when it sees the switch off first.
        if stop.load(Ordering::SeqCst)
            && !self.extractor.guild().extraction_enabled()
            && let Some(tracked) = self.state().jobs.get_mut(id)
        {
            tracked
                .reason
                .get_or_insert_with(|| SWITCHED_OFF.to_owned());
        }
        // A stop is applied by `finish`, under the lock `cancel` reads.
        let status = if failure.is_some() && results.is_empty() {
            RescanStatus::Failed
        } else {
            RescanStatus::Done
        };
        self.finish(&mut job, status, failure).await;
    }

    fn set_current(&self, id: &str, current: Option<String>) {
        if let Some(tracked) = self.state().jobs.get_mut(id) {
            tracked.current = current;
        }
    }

    fn set_results(&self, id: &str, results: Value) {
        if let Some(tracked) = self.state().jobs.get_mut(id) {
            tracked.job.results = results;
        }
    }

    async fn finish(&self, job: &mut RescanJob, status: RescanStatus, error: Option<String>) {
        job.status = status;
        job.error = error;
        job.finished_at = Some(self.extractor.now());
        self.state().settle(job);
        let _ = self.extractor.store().update_rescan_job(job.clone()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn running(state: &mut State) -> RescanJob {
        let job = RescanJob {
            id: "job".into(),
            channels: vec!["900".into()],
            window: "week".into(),
            source: "portal".into(),
            automated: false,
            requested_by: None,
            status: RescanStatus::Running,
            created_at: Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap(),
            started_at: None,
            finished_at: None,
            results: Value::Array(Vec::new()),
            error: None,
        };
        state.remember(Tracked {
            job: job.clone(),
            stop: Arc::new(AtomicBool::new(false)),
            current: Some("900".into()),
            reason: None,
            unprocessed_only: false,
        });
        job
    }

    /// Both orders of a cancel racing the worker's last step: an accepted
    /// stop always ends `cancelled`, and once `done` is latched a cancel is
    /// refused, so a cancel answered `true` can never poll `done`.
    #[test]
    fn a_cancel_and_the_final_status_never_disagree() {
        let mut state = State::default();
        let mut job = running(&mut state);
        assert!(state.stop_running("job"));
        job.status = RescanStatus::Done;
        state.settle(&mut job);
        assert_eq!(job.status, RescanStatus::Cancelled);
        assert_eq!(
            state.view("job").unwrap().job.status,
            RescanStatus::Cancelled
        );

        let mut state = State::default();
        let mut job = running(&mut state);
        job.status = RescanStatus::Done;
        state.settle(&mut job);
        assert!(!state.stop_running("job"));
        assert_eq!(state.view("job").unwrap().job.status, RescanStatus::Done);
        assert!(!state.stop_running("unknown"));
    }
}
