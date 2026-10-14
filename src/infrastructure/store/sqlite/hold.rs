//! Test support: SQLite runs on sqlx's worker threads, so a store call on a
//! paused tokio clock leaves the runtime idle, and the clock auto-advances
//! to the next timer (the 5 s reader acquire timeout, a debounce, a
//! deadline) before SQLite answers. Tokio does not auto-advance while a
//! `spawn_blocking` task runs, so a [`ClockHold`] parks one for as long as
//! the store waits on SQLite: a writer lease, a reader, open and close.

use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::sync::oneshot;

/// Keeps a paused clock from auto-advancing until dropped. Free outside a
/// current-thread runtime, the only kind whose clock can be paused.
#[must_use]
pub(super) struct ClockHold {
    _release: Option<oneshot::Sender<()>>,
}

impl ClockHold {
    pub(super) fn new() -> Self {
        let Ok(runtime) = Handle::try_current() else {
            return Self { _release: None };
        };
        if runtime.runtime_flavor() != RuntimeFlavor::CurrentThread {
            return Self { _release: None };
        }
        let (release, parked) = oneshot::channel::<()>();
        // Dropping the sender wakes it; the task ending lets the clock go.
        drop(runtime.spawn_blocking(move || {
            let _ = parked.blocking_recv();
        }));
        Self {
            _release: Some(release),
        }
    }
}
