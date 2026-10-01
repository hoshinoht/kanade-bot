//! The live loop: message events in, 90 s per-channel debounce (edits push it
//! out again, a ping with a boss or time flushes at once), and the backlog
//! drained one burst per interval. Bursts run as tasks so events keep flowing
//! while a model call is in flight; the governor serialises the calls.

use std::future::pending;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

use super::debounce::Bursts;
use super::extractor::{Extractor, PassReport};
use super::ports::{BacklogDrop, IncomingMessage, MessageEvent, MessageOrigin, Outbox, Proposer};
use crate::domain::model_log::{MessageUpsert, ModelLogStore};
use crate::domain::scheduler::ScheduleStore;
use crate::extract::backlog::{Backlog, BacklogEntry};
use crate::extract::gate;
use crate::infrastructure::llm::LlmProvider;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Live,
    Backlog,
}

struct Done {
    source: Source,
    report: PassReport,
}

/// Owns the debounce buffers and the backlog; [`Pipeline::run`] until the
/// event channel closes.
pub struct Pipeline<S, P, X, O> {
    extractor: Arc<Extractor<S, P, X, O>>,
    bursts: Bursts,
    backlog: Backlog,
    tasks: JoinSet<Done>,
    draining: bool,
    next_drain: Instant,
    /// Rows another pass released unread after this loop wanted them.
    reoffers: Option<mpsc::UnboundedReceiver<BacklogEntry>>,
}

async fn reoffered(reoffers: &mut Option<mpsc::UnboundedReceiver<BacklogEntry>>) -> BacklogEntry {
    match reoffers {
        Some(receiver) => match receiver.recv().await {
            Some(entry) => entry,
            None => pending().await,
        },
        None => pending().await,
    }
}

async fn at(when: Option<Instant>) {
    match when {
        Some(when) => sleep_until(when).await,
        None => pending().await,
    }
}

fn entry(message: &IncomingMessage) -> BacklogEntry {
    BacklogEntry {
        channel_id: message.channel_id.clone(),
        message_id: message.id.clone(),
        created_at: message.created_at,
    }
}

impl<S, P, X, O> Pipeline<S, P, X, O>
where
    S: ScheduleStore + ModelLogStore + Send + Sync + 'static,
    P: LlmProvider + 'static,
    X: Proposer + 'static,
    O: Outbox + 'static,
{
    pub fn new(extractor: Arc<Extractor<S, P, X, O>>) -> Self {
        let capacity = extractor.config().backlog_capacity;
        let reoffers = extractor.claims().take_reoffers();
        Self {
            extractor,
            bursts: Bursts::new(),
            backlog: Backlog::new(capacity),
            tasks: JoinSet::new(),
            draining: false,
            next_drain: Instant::now(),
            reoffers,
        }
    }

    /// Process events until `events` closes, then wait for bursts in flight.
    /// Buffered bursts are dropped on shutdown; their messages stay
    /// unprocessed in the cache for the next rescan or backfill (v4).
    pub async fn run(mut self, mut events: mpsc::Receiver<MessageEvent>) {
        loop {
            let debounce = self.bursts.next_deadline();
            let drain = (!self.draining && !self.backlog.is_empty()).then_some(self.next_drain);
            tokio::select! {
                biased;
                event = events.recv() => match event {
                    Some(event) => self.on_event(event).await,
                    None => break,
                },
                Some(done) = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    self.finished(done.ok()).await;
                }
                () = at(debounce) => {
                    for (channel, burst) in self.bursts.due(Instant::now()) {
                        self.spawn(Source::Live, channel, burst);
                    }
                }
                () = at(drain) => self.drain_one(),
                entry = reoffered(&mut self.reoffers) => self.reoffer(entry).await,
            }
        }
        self.bursts.drain();
        while let Some(done) = self.tasks.join_next().await {
            self.finished(done.ok()).await;
        }
    }

    async fn on_event(&mut self, event: MessageEvent) {
        let (message, edited) = match event {
            MessageEvent::Posted(message) => (message, false),
            MessageEvent::Edited(message) => (message, true),
            MessageEvent::Deleted { id } => {
                // A failed delete leaves a cached row the next read re-gates.
                let _ = self.extractor.store.delete_message(&id).await;
                self.bursts.forget(&id);
                self.backlog.remove(&id);
                return;
            }
        };
        let stored = match self.extractor.store_message(&message).await {
            Ok(Some(stored)) => stored,
            // Not a member's message in a watched channel, or the cache is
            // down: nothing to read it from later either.
            Ok(None) | Err(_) => return,
        };
        if stored == MessageUpsert::Unchanged {
            return;
        }
        let Some(result) = self.extractor.gate_incoming(&message) else {
            return;
        };
        // One owner per id: a pending live burst keeps a replayed message, and
        // live activity takes a message out of the backlog.
        if message.origin == MessageOrigin::Replay && !self.bursts.contains(&message.id) {
            let dropped = self.backlog.push(entry(&message));
            self.audit(dropped).await;
            return;
        }
        self.backlog.remove(&message.id);
        let now = Instant::now();
        self.bursts
            .add(entry(&message), now, self.extractor.config().debounce);
        if !edited
            && message.origin == MessageOrigin::Live
            && gate::urgent(&result)
            && let Some(burst) = self.bursts.take(&message.channel_id)
        {
            self.spawn(Source::Live, message.channel_id.clone(), burst);
        }
    }

    /// A deferred row whose reader let go of it unread: the backlog reads it
    /// (a pending burst already holding it reads it anyway).
    async fn reoffer(&mut self, entry: BacklogEntry) {
        if self.bursts.contains(&entry.message_id) {
            return;
        }
        let dropped = self.backlog.push(entry);
        self.audit(dropped).await;
    }

    fn spawn(&mut self, source: Source, channel: String, burst: Vec<BacklogEntry>) {
        let extractor = self.extractor.clone();
        self.tasks.spawn(async move {
            let report = extractor.flush(&channel, &burst).await;
            Done { source, report }
        });
    }

    fn drain_one(&mut self) {
        if let Some((channel, burst)) = self.backlog.next_burst() {
            self.draining = true;
            self.spawn(Source::Backlog, channel, burst);
        }
    }

    async fn finished(&mut self, done: Option<Done>) {
        let Some(Done { source, report }) = done else {
            // A panicked burst: its messages stay unprocessed in the cache.
            self.draining = false;
            self.next_drain = Instant::now() + self.extractor.config().drain_interval;
            return;
        };
        let turned_away = !report.turned_away.is_empty();
        // Edited meanwhile: the pending burst reads it again, not the backlog.
        let requeue: Vec<BacklogEntry> = report
            .turned_away
            .into_iter()
            .filter(|entry| !self.bursts.contains(&entry.message_id))
            .collect();
        let dropped = match source {
            Source::Live => requeue
                .into_iter()
                .flat_map(|entry| self.backlog.push(entry))
                .collect(),
            Source::Backlog => self.backlog.requeue(requeue),
        };
        self.audit(dropped).await;
        let paced = Instant::now() + self.extractor.config().drain_interval;
        // Breaker open or rate-limited: wait for the governor, never spin.
        let wait = match report.retry_at {
            Some(retry_at) if turned_away => paced.max(retry_at),
            _ => paced,
        };
        match source {
            Source::Backlog => {
                self.draining = false;
                // Keep a later hold a live refusal set while this drain ran.
                self.next_drain = self.next_drain.max(wait);
            }
            Source::Live if turned_away => self.next_drain = self.next_drain.max(wait),
            Source::Live => {}
        }
    }

    async fn audit(&self, dropped: Vec<BacklogEntry>) {
        if dropped.is_empty() {
            return;
        }
        let drop = BacklogDrop {
            message_ids: dropped.into_iter().map(|entry| entry.message_id).collect(),
            capacity: self.backlog.capacity(),
        };
        self.extractor.outbox.backlog_dropped(drop).await;
    }
}
