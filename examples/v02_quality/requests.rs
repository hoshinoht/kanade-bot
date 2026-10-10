//! Every request the model provider sends passes this counter (installed as
//! the provider's test-support request hook): it counts listings and
//! completions alike, refuses everything once the cap is reached, and
//! remembers any peer that is not loopback.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use kanade::infrastructure::llm::{SentRequest, set_request_hook};

/// The user's hard ceiling for one run (2026-10-07).
pub const HARD_CAP: usize = 500;

#[derive(Default)]
pub struct Requests {
    cap: AtomicUsize,
    sent: AtomicUsize,
    refused: AtomicUsize,
    by_path: Mutex<BTreeMap<String, usize>>,
    off_loopback: Mutex<Vec<String>>,
}

impl Requests {
    pub fn install(cap: usize) -> Arc<Self> {
        let requests = Arc::new(Self::default());
        requests.cap.store(cap, Ordering::SeqCst);
        let seen = Arc::clone(&requests);
        set_request_hook(Some(Arc::new(move |request: &SentRequest<'_>| {
            seen.admit(request)
        })));
        requests
    }

    fn admit(&self, request: &SentRequest<'_>) -> bool {
        let cap = self.cap.load(Ordering::SeqCst);
        // Reserve a slot atomically so parallel requests never overshoot.
        let admitted = self
            .sent
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |sent| {
                (sent < cap).then_some(sent + 1)
            })
            .is_ok();
        if !admitted {
            self.refused.fetch_add(1, Ordering::SeqCst);
            return false;
        }
        let key = format!("{} {}", request.method, request.path);
        *self
            .by_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(key)
            .or_default() += 1;
        if !request.peer.is_some_and(|peer| peer.ip().is_loopback()) {
            self.off_loopback
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{:?}", request.peer));
        }
        true
    }

    pub fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }

    pub fn refused(&self) -> usize {
        self.refused.load(Ordering::SeqCst)
    }

    pub fn cap(&self) -> usize {
        self.cap.load(Ordering::SeqCst)
    }

    pub fn exhausted(&self) -> bool {
        self.sent() >= self.cap() || self.refused() > 0
    }

    pub fn completions(&self) -> usize {
        self.by_path()
            .iter()
            .filter(|(path, _)| path.ends_with("/chat/completions"))
            .map(|(_, count)| count)
            .sum()
    }

    pub fn by_path(&self) -> BTreeMap<String, usize> {
        self.by_path
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn off_loopback(&self) -> Vec<String> {
        self.off_loopback
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
