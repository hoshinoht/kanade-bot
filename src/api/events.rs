//! The change-hint hub behind `GET /api/admin/events` and the member stream
//! `GET /api/public/events`. The store's write hook feeds it (every write path
//! commits through the one store), and each open admin stream hears
//! `{topic, seq}` only; member streams turn the same hints into their own
//! member-scoped topics. Streams are bounded (members apart from admins, and
//! per member), and [`Hub::close`] ends them all so graceful shutdown never
//! waits on one.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use tokio::sync::{OwnedSemaphorePermit, Semaphore, broadcast, watch};

use super::dto::events::{EventHint, Topic};
use crate::infrastructure::store::{WriteObserver, Written};

/// Hints a slow stream may fall behind by before it skips ahead (the client
/// sees the `seq` gap and re-reads everything).
const BACKLOG: usize = 64;

/// Member streams one member may hold open at once (tabs and devices together).
pub const MEMBER_STREAMS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventsConfig {
    /// A comment line this often keeps proxies from idling the stream out and
    /// re-checks the session.
    pub heartbeat: Duration,
    /// Open admin streams at most; one more is refused.
    pub max_clients: usize,
    /// Open member streams at most, all members together; one more is
    /// refused. Apart from the admin cap, so members never crowd admins out.
    pub max_member_clients: usize,
    /// A stream ends after this long and the browser reconnects, so the full
    /// sign-in checks (staff, edge identity) run again as on any request.
    pub max_lifetime: Duration,
}

impl Default for EventsConfig {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(20),
            max_clients: 32,
            max_member_clients: 64,
            max_lifetime: Duration::from_secs(5 * 60),
        }
    }
}

/// Open member streams by member id.
type Holders = Arc<Mutex<HashMap<String, usize>>>;

pub struct Hub {
    config: EventsConfig,
    sender: broadcast::Sender<EventHint>,
    /// The last hint's seq; held while sending so seq order is send order.
    last: Mutex<u64>,
    clients: Arc<Semaphore>,
    member_clients: Arc<Semaphore>,
    holders: Holders,
    closed: watch::Sender<bool>,
    /// Unique per process, so clients can tell a restart from a reconnect.
    boot: String,
}

impl std::fmt::Debug for Hub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hub")
            .field("config", &self.config)
            .field("streams", &self.streams())
            .field("member_streams", &self.member_streams())
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
    /// This process's id (`EventReady::boot`).
    pub boot: String,
    pub closed: watch::Receiver<bool>,
}

/// One open member stream's share of the hub: the hub's hints (kinds only;
/// the stream decides what each means for its member) and no `seq`, which
/// would count every write, the admins' included.
pub(crate) struct MemberSubscription {
    /// Released when the stream ends.
    pub slot: MemberSlot,
    pub hints: broadcast::Receiver<EventHint>,
    pub boot: String,
    pub closed: watch::Receiver<bool>,
}

/// A member stream's place under both caps; dropping it frees both.
pub(crate) struct MemberSlot {
    _permit: OwnedSemaphorePermit,
    holders: Holders,
    member: String,
}

impl Drop for MemberSlot {
    fn drop(&mut self) {
        let mut holders = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(open) = holders.get_mut(&self.member) {
            *open -= 1;
            if *open == 0 {
                holders.remove(&self.member);
            }
        }
    }
}

impl Hub {
    pub fn new(config: EventsConfig) -> Self {
        Self {
            config,
            sender: broadcast::channel(BACKLOG).0,
            last: Mutex::new(0),
            clients: Arc::new(Semaphore::new(config.max_clients)),
            member_clients: Arc::new(Semaphore::new(config.max_member_clients)),
            holders: Holders::default(),
            closed: watch::channel(false).0,
            boot: uuid::Uuid::new_v4().simple().to_string(),
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
        self.member_clients.close();
    }

    /// Admin streams open now.
    pub fn streams(&self) -> usize {
        self.config
            .max_clients
            .saturating_sub(self.clients.available_permits())
    }

    /// Member streams open now, all members together.
    pub fn member_streams(&self) -> usize {
        self.config
            .max_member_clients
            .saturating_sub(self.member_clients.available_permits())
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
            boot: self.boot.clone(),
            closed: self.closed.subscribe(),
        })
    }

    /// `None` at either cap ([`MEMBER_STREAMS`] for `member`, or all members'
    /// [`EventsConfig::max_member_clients`]) or after [`Hub::close`].
    pub(crate) fn subscribe_member(&self, member: &str) -> Option<MemberSubscription> {
        let mut holders = self.holders.lock().unwrap_or_else(PoisonError::into_inner);
        let open = holders.get(member).copied().unwrap_or(0);
        if open >= MEMBER_STREAMS {
            return None;
        }
        let permit = Arc::clone(&self.member_clients).try_acquire_owned().ok()?;
        holders.insert(member.to_owned(), open + 1);
        Some(MemberSubscription {
            slot: MemberSlot {
                _permit: permit,
                holders: Arc::clone(&self.holders),
                member: member.to_owned(),
            },
            hints: self.sender.subscribe(),
            boot: self.boot.clone(),
            closed: self.closed.subscribe(),
        })
    }
}
