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
    /// A close frame arrived; Twilight is reconnecting (or the gateway ended).
    Disconnected,
}

impl Connection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Ready => "ready",
            Self::Disconnected => "disconnected",
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
            _ => Connection::Connecting,
        }
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
