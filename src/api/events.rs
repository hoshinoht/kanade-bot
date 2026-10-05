//! The admin change-hint hub behind `GET /api/admin/events`. The store's
//! write hook feeds it (every write path commits through the one store), and
//! each open stream hears `{topic, seq}` only. Streams are bounded, and
//! [`Hub::close`] ends them all so graceful shutdown never waits on one.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, watch};

use super::dto::events::{EventHint, Topic};
use crate::infrastructure::store::{WriteObserver, Written};

/// Hints a slow stream may fall behind by before it skips ahead (the client
/// sees the `seq` gap and re-reads everything).
const BACKLOG: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventsConfig {
    /// A comment line this often keeps proxies from idling the stream out and
    /// re-checks the session.
    pub heartbeat: Duration,
    /// Open streams at most; one more is refused.
    pub max_clients: usize,
    /// A stream ends after this long and the browser reconnects, so the full
    /// sign-in checks (staff, edge identity) run again as on any request.
    pub max_lifetime: Duration,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(20),
            max_clients: 32,
            max_lifetime: Duration::from_secs(5 * 60),
        }
    }
}

pub struct Hub {
    config: EventsConfig,
    sender: broadcast::Sender<EventHint>,
    /// The last hint's seq; held while sending so seq order is send order.
    last: Mutex<u64>,
    clients: Arc<Semaphore>,
    closed: watch::Sender<bool>,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("config", &self.config)
            .field("streams", &self.streams())
            .finish_non_exhaustive()
    }
}

impl Default for Hub {
    fn default() -> Self {
        Self::new(EventsConfig::default())
    }
}

/// One open stream's share of the hub.
pub(crate) struct Subscription {
    /// Released when the stream ends.
    pub permit: OwnedSemaphorePermit,
    pub hints: broadcast::Receiver<EventHint>,
    /// The seq before the first hint this stream will hear.
    pub ready: u64,
    pub closed: watch::Receiver<bool>,
}

impl Hub {
    pub fn new(config: EventsConfig) -> Self {
        Self {
            config,
            sender: broadcast::channel(BACKLOG).0,
            last: Mutex::new(0),
            clients: Arc::new(Semaphore::new(config.max_clients)),
            closed: watch::channel(false).0,
        }
    }

    pub fn config(&self) -> EventsConfig {
        self.config
    }

    pub fn notify(&self, topic: Topic) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        *last += 1;
        // No stream open is not an error: the hint simply has no listener.
        let _ = self.sender.send(EventHint { topic, seq: *last });
    }

    /// The store's write hook: cheap and synchronous, as the store requires.
    pub fn observer(self: &Arc<Self>) -> WriteObserver {
        let hub = Arc::clone(self);
        Arc::new(move |written: Written| hub.notify(written.into()))
    }

    /// End every open stream and refuse new ones (graceful shutdown).
    pub fn close(&self) {
        self.closed.send_replace(true);
        self.clients.close();
    }

    /// Streams open now.
    pub fn streams(&self) -> usize {
        self.config
            .max_clients
            .saturating_sub(self.clients.available_permits())
    }

    /// Whether [`Hub::close`] ran (shutting down).
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// `None` at the cap or after [`Hub::close`].
    pub(crate) fn subscribe(&self) -> Option<Subscription> {
        let permit = Arc::clone(&self.clients).try_acquire_owned().ok()?;
        let last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        Some(Subscription {
            permit,
            hints: self.sender.subscribe(),
            ready: *last,
            closed: self.closed.subscribe(),
        })
    }
}
