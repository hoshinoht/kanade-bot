//! A scripted rescan runner: `submit` queues a job and spawns its worker
//! task, which moves one step (start, then one channel per step) each time
//! the test calls [`FakeRescans::step`].

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{Arc, Mutex},
};

use chrono::{DateTime, Utc};
use kanade::{
    api::rescan::{RescanFuture, RescanRunner, RescanView},
    domain::model_log::{RescanJob, RescanStatus},
    extract::rescan::{RescanError, RescanRequest},
};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc};

struct Tracked {
    view: RescanView,
    permits: Arc<Semaphore>,
    stepped: Option<mpsc::UnboundedReceiver<()>>,
}

pub struct FakeRescans {
    jobs: Mutex<BTreeMap<String, Tracked>>,
    /// Every request `submit` received.
    pub requests: Mutex<Vec<RescanRequest>>,
    /// Per-channel results to report instead of the default.
    pub results: Mutex<BTreeMap<String, Value>>,
    /// Channels whose read fails outright (no result).
    pub failing: Mutex<BTreeSet<String>>,
    /// Per-channel counts a job expects when it starts; unlisted channels
    /// have none.
    pub expected: Mutex<HashMap<String, usize>>,
    /// Refuse submits as if extraction were switched off.
    pub off: Mutex<bool>,
    at: DateTime<Utc>,
    me: std::sync::Weak<FakeRescans>,
}

impl FakeRescans {
    pub fn new(at: DateTime<Utc>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            jobs: Mutex::new(BTreeMap::new()),
            requests: Mutex::new(Vec::new()),
            results: Mutex::new(BTreeMap::new()),
            failing: Mutex::new(BTreeSet::new()),
            expected: Mutex::new(HashMap::new()),
            off: Mutex::new(false),
            at,
            me: me.clone(),
        })
    }

    /// Let job `id`'s worker take one step and wait until it has.
    pub async fn step(&self, id: &str) {
        let (permits, mut stepped) = {
            let mut jobs = self.jobs.lock().unwrap();
            let tracked = jobs.get_mut(id).expect("known job");
            (tracked.permits.clone(), tracked.stepped.take().unwrap())
        };
        permits.add_permits(1);
        stepped.recv().await.expect("worker alive");
        self.jobs.lock().unwrap().get_mut(id).unwrap().stepped = Some(stepped);
    }

    pub fn status(&self, id: &str) -> RescanStatus {
        self.jobs.lock().unwrap()[id].view.job.status
    }

    fn advance(&self, id: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        let view = &mut jobs.get_mut(id).unwrap().view;
        let job = &mut view.job;
        if job.status.is_final() {
            return;
        }
        if view.stopping {
            job.status = RescanStatus::Cancelled;
            view.stopping = false;
            view.current = None;
            return;
        }
        if job.status == RescanStatus::Queued {
            job.status = RescanStatus::Running;
            job.started_at = Some(self.at);
            view.current = job.channels.first().cloned();
            view.expected = self.expected.lock().unwrap().clone();
            return;
        }
        let Some(channel) = view.current.take() else {
            return;
        };
        if !self.failing.lock().unwrap().contains(&channel) {
            let result = self
                .results
                .lock()
                .unwrap()
                .get(&channel)
                .cloned()
                .unwrap_or_else(|| {
                    json!({"channel_id": channel, "name": null, "gated": 3, "proposals": 1,
                           "unread": 0, "errors": []})
                });
            job.results.as_array_mut().unwrap().push(result);
        }
        let index = job.channels.iter().position(|c| *c == channel).unwrap();
        view.current = job.channels.get(index + 1).cloned();
        if view.current.is_none() {
            job.status = RescanStatus::Done;
        }
    }
}

impl RescanRunner for FakeRescans {
    fn ready(&self) -> Result<(), RescanError> {
        if *self.off.lock().unwrap() {
            Err(RescanError::Off)
        } else {
            Ok(())
        }
    }

    fn submit(&self, request: RescanRequest) -> RescanFuture<'_, RescanView> {
        Box::pin(async move {
            self.ready()?;
            self.requests.lock().unwrap().push(request.clone());
            let mut jobs = self.jobs.lock().unwrap();
            let id = format!("job-{}", jobs.len() + 1);
            let permits = Arc::new(Semaphore::new(0));
            let (tx, rx) = mpsc::unbounded_channel();
            let view = RescanView {
                job: RescanJob {
                    id: id.clone(),
                    channels: request.channels,
                    window: request.window,
                    source: request.source,
                    automated: request.automated,
                    requested_by: request.requested_by,
                    status: RescanStatus::Queued,
                    created_at: self.at,
                    started_at: None,
                    finished_at: None,
                    results: json!([]),
                    error: None,
                },
                current: None,
                expected: HashMap::new(),
                stopping: false,
            };
            jobs.insert(
                id.clone(),
                Tracked {
                    view: view.clone(),
                    permits: permits.clone(),
                    stepped: Some(rx),
                },
            );
            let me = self.me.upgrade().unwrap();
            tokio::spawn(async move {
                loop {
                    permits.acquire().await.unwrap().forget();
                    me.advance(&id);
                    if tx.send(()).is_err() {
                        return;
                    }
                }
            });
            Ok(view)
        })
    }

    fn job(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        Box::pin(async move {
            Ok(self
                .jobs
                .lock()
                .unwrap()
                .get(&id)
                .map(|tracked| tracked.view.clone()))
        })
    }

    fn cancel(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        Box::pin(async move {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(tracked) = jobs.get_mut(&id) else {
                return Ok(None);
            };
            let view = &mut tracked.view;
            match view.job.status {
                RescanStatus::Queued => view.job.status = RescanStatus::Cancelled,
                RescanStatus::Running => view.stopping = true,
                _ => {}
            }
            Ok(Some(view.clone()))
        })
    }
}
