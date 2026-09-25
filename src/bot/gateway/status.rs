//! The gateway connection state, shared with health.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use twilight_gateway::Event;

/// Where the session stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    /// Before the first `READY`, or reconnecting after a fresh identify.
    Connecting,
    /// `READY` or `RESUMED` arrived since the last close.
    Ready,
    /// A close frame arrived or a reconnect failed; Twilight is reconnecting.
    Disconnected,
    /// A fatal close ended the session for good (serve does not reconnect).
    Closed,
}

impl Connection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Disconnected => "disconnected",
            Self::Closed => "closed",
        }
    }
}

/// A cloneable handle the runner updates from raw gateway events.
#[derive(Clone, Debug, Default)]
pub struct ConnectionStatus(Arc<AtomicU8>);

impl ConnectionStatus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self) -> Connection {
        match self.0.load(Ordering::Relaxed) {
            1 => Connection::Ready,
            2 => Connection::Disconnected,
            3 => Connection::Closed,
            _ => Connection::Connecting,
        }
    }

    /// The session ended for good; later events cannot revive it.
    pub fn closed(&self) {
        self.0.store(3, Ordering::Relaxed);
    }

    /// Only a ready session drops to disconnected; connecting stays so.
    pub(super) fn disconnected(&self) {
        let _ = self
            .0
            .compare_exchange(1, 2, Ordering::Relaxed, Ordering::Relaxed);
    }

    pub(super) fn observe(&self, event: &Event) {
        let next = match event {
            Event::Ready(_) | Event::Resumed => 1,
            Event::GatewayClose(_) => 2,
            _ => return,
        };
        self.0.store(next, Ordering::Relaxed);
    }
}
