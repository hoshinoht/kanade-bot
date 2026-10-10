//! One model admission per (message id, content version) across live bursts,
//! backlog drains and rescans that share an [`Extractor`](super::Extractor).
//! A pass claims its rows before any call and holds them until its commit
//! returns; a pass that finds a row claimed drops it (never waits) and marks
//! the claim contended, so the owner re-offers the row to the live backlog
//! if it releases without marking it processed.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::mpsc;

use crate::domain::model_log::{ReadMessage, WatchedMessage};
use crate::extract::backlog::BacklogEntry;

/// Claims held at once; past it new rows are deferred (fail closed).
pub const CLAIM_CAPACITY: usize = 10_000;

/// A message at one content: an edit is a new version with its own claim.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    id: String,
    hash: u64,
    len: usize,
}

impl Key {
    fn new(id: &str, content: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        Self {
            id: id.to_owned(),
            hash: hasher.finish(),
            len: content.len(),
        }
    }
}

#[derive(Debug)]
struct Entry {
    token: u64,
    contended: bool,
}

pub struct Claims {
    entries: Mutex<HashMap<Key, Entry>>,
    next: AtomicU64,
    capacity: usize,
    reoffer: mpsc::UnboundedSender<BacklogEntry>,
    reoffers: Mutex<Option<mpsc::UnboundedReceiver<BacklogEntry>>>,
}

impl std::fmt::Debug for Claims {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claims")
            .field("held", &self.entries().len())
            .finish_non_exhaustive()
    }
}

impl Default for Claims {
    fn default() -> Self {
        Self::with_capacity(CLAIM_CAPACITY)
    }
}

impl Claims {
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        let (reoffer, reoffers) = mpsc::unbounded_channel();
        Self {
            entries: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            capacity,
            reoffer,
            reoffers: Mutex::new(Some(reoffers)),
        }
    }

    /// A panicking holder never leaves the map unusable.
    fn entries(&self) -> MutexGuard<'_, HashMap<Key, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Rows released unread after another pass wanted them; taken once, by
    /// the live loop that owns the backlog.
    pub fn take_reoffers(&self) -> Option<mpsc::UnboundedReceiver<BacklogEntry>> {
        self.reoffers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// An empty hold with its own token.
    pub fn hold(&self) -> ClaimGuard<'_> {
        ClaimGuard {
            claims: self,
            token: self.next.fetch_add(1, Ordering::Relaxed),
            held: Vec::new(),
            marked: HashSet::new(),
            retry: HashSet::new(),
        }
    }

    /// How many claims are held (tests and diagnostics).
    pub fn held(&self) -> usize {
        self.entries().len()
    }

    fn reoffer(&self, entry: BacklogEntry) {
        // No live loop (a rescan-only setup): the next rescan reads it.
        let _ = self.reoffer.send(entry);
    }
}

/// The rows one pass owns. Dropping it (commit returned, a panic, an abort)
/// releases only entries still carrying its token.
pub struct ClaimGuard<'a> {
    claims: &'a Claims,
    token: u64,
    held: Vec<(Key, BacklogEntry)>,
    marked: HashSet<Key>,
    retry: HashSet<Key>,
}

impl std::fmt::Debug for ClaimGuard<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimGuard")
            .field("token", &self.token)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

impl ClaimGuard<'_> {
    /// Claim `rows`; returns the ones this pass now owns (already owned ones
    /// included) and how many were deferred: claimed by another pass
    /// (marked contended) or refused because the table is full (re-offered).
    pub fn claim(&mut self, rows: Vec<WatchedMessage>) -> (Vec<WatchedMessage>, usize) {
        let mut won = Vec::with_capacity(rows.len());
        let mut deferred = 0;
        let mut full = Vec::new();
        {
            let mut entries = self.claims.entries();
            for row in rows {
                let key = Key::new(&row.id, &row.content);
                let full_table = entries.len() >= self.claims.capacity;
                match entries.get_mut(&key) {
                    Some(entry) if entry.token == self.token => won.push(row),
                    Some(entry) => {
                        entry.contended = true;
                        deferred += 1;
                    }
                    None if full_table => {
                        full.push(super::extractor::entry(&row));
                        deferred += 1;
                    }
                    None => {
                        entries.insert(
                            key.clone(),
                            Entry {
                                token: self.token,
                                contended: false,
                            },
                        );
                        self.held.push((key, super::extractor::entry(&row)));
                        won.push(row);
                    }
                }
            }
        }
        for entry in full {
            self.claims.reoffer(entry);
        }
        (won, deferred)
    }

    /// These reads were marked processed: nothing to re-offer for them.
    pub fn marked(&mut self, read: &[ReadMessage]) {
        for message in read {
            let key = Key::new(&message.id, &message.content);
            self.retry.remove(&key);
            self.marked.insert(key);
        }
    }

    /// Re-offer these reads on release even if nobody else wanted them (a
    /// processed mark that could not be written).
    pub fn retry(&mut self, read: &[ReadMessage]) {
        for message in read {
            let key = Key::new(&message.id, &message.content);
            if !self.marked.contains(&key) {
                self.retry.insert(key);
            }
        }
    }
}

impl Drop for ClaimGuard<'_> {
    fn drop(&mut self) {
        let mut reoffer = Vec::new();
        {
            let mut entries = self.claims.entries();
            for (key, entry) in self.held.drain(..) {
                let Some(held) = entries.get(&key) else {
                    continue;
                };
                if held.token != self.token {
                    continue;
                }
                let contended = held.contended;
                entries.remove(&key);
                let unread = !self.marked.contains(&key);
                if unread && (contended || self.retry.contains(&key)) {
                    reoffer.push(entry);
                }
            }
        }
        for entry in reoffer {
            self.claims.reoffer(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn row(id: &str, content: &str) -> WatchedMessage {
        WatchedMessage {
            id: id.into(),
            channel_id: "900".into(),
            author_id: "1".into(),
            created_at: Utc.with_ymd_and_hms(2026, 8, 30, 5, 0, 0).unwrap(),
            edited_at: None,
            content: content.into(),
            processed_at: None,
        }
    }

    fn read(id: &str, content: &str) -> ReadMessage {
        ReadMessage {
            id: id.into(),
            content: content.into(),
        }
    }

    fn drained(receiver: &mut mpsc::UnboundedReceiver<BacklogEntry>) -> Vec<String> {
        let mut ids = Vec::new();
        while let Ok(entry) = receiver.try_recv() {
            ids.push(entry.message_id);
        }
        ids
    }

    #[test]
    fn a_second_pass_is_deferred_and_the_version_is_its_own_claim() {
        let claims = Claims::default();
        let mut first = claims.hold();
        let (won, deferred) = first.claim(vec![row("1", "hstar 9pm")]);
        assert_eq!((won.len(), deferred), (1, 0));
        // Claiming again under the same hold keeps it.
        assert_eq!(first.claim(vec![row("1", "hstar 9pm")]).0.len(), 1);
        let mut second = claims.hold();
        let (won, deferred) = second.claim(vec![row("1", "hstar 9pm"), row("1", "hstar 10pm")]);
        assert_eq!(deferred, 1);
        assert_eq!(won.len(), 1, "the edit is a new version");
        assert_eq!(won[0].content, "hstar 10pm");
        drop(first);
        drop(second);
        assert_eq!(claims.held(), 0);
    }

    #[test]
    fn release_reoffers_only_contended_unmarked_rows() {
        let claims = Claims::default();
        let mut receiver = claims.take_reoffers().expect("first take");
        assert!(claims.take_reoffers().is_none(), "taken once");

        // Uncontended and unmarked: nobody wanted it, nothing re-offered.
        drop({
            let mut hold = claims.hold();
            hold.claim(vec![row("1", "a")]);
            hold
        });
        assert!(drained(&mut receiver).is_empty());

        // Contended and marked: read, nothing re-offered.
        let mut owner = claims.hold();
        owner.claim(vec![row("2", "b"), row("3", "c")]);
        let mut loser = claims.hold();
        assert_eq!(loser.claim(vec![row("2", "b"), row("3", "c")]).1, 2);
        owner.marked(&[read("2", "b")]);
        drop(owner);
        assert_eq!(drained(&mut receiver), ["3"], "contended and unread");
        drop(loser);
        assert!(drained(&mut receiver).is_empty(), "the loser held nothing");

        // Forced retry after an unwritable mark.
        let mut hold = claims.hold();
        hold.claim(vec![row("4", "d")]);
        hold.retry(&[read("4", "d")]);
        drop(hold);
        assert_eq!(drained(&mut receiver), ["4"]);
        assert_eq!(claims.held(), 0);
    }

    #[test]
    fn a_release_never_frees_another_holders_entry() {
        let claims = Claims::default();
        let mut owner = claims.hold();
        owner.claim(vec![row("1", "a")]);
        // A forged hold listing the same key under another token.
        let mut stranger = claims.hold();
        stranger.held.push((
            Key::new("1", "a"),
            super::super::extractor::entry(&row("1", "a")),
        ));
        drop(stranger);
        assert_eq!(claims.held(), 1, "the owner's entry survives");
        let mut late = claims.hold();
        assert_eq!(late.claim(vec![row("1", "a")]).1, 1);
        drop(owner);
        assert_eq!(claims.held(), 0);
    }

    #[test]
    fn a_full_table_defers_new_rows_and_reoffers_them() {
        let claims = Claims::with_capacity(1);
        let mut receiver = claims.take_reoffers().expect("take");
        let mut first = claims.hold();
        assert_eq!(first.claim(vec![row("1", "a")]).1, 0);
        let mut second = claims.hold();
        let (won, deferred) = second.claim(vec![row("2", "b")]);
        assert!(won.is_empty());
        assert_eq!(deferred, 1);
        assert_eq!(drained(&mut receiver), ["2"]);
        drop(first);
        assert_eq!(second.claim(vec![row("2", "b")]).0.len(), 1);
    }

    #[test]
    fn a_poisoned_table_still_claims_and_releases() {
        let claims = Claims::default();
        std::thread::scope(|scope| {
            let _ = scope
                .spawn(|| {
                    let _entries = claims.entries();
                    panic!("poison the claims");
                })
                .join();
        });
        assert!(claims.entries.is_poisoned());
        let mut hold = claims.hold();
        assert_eq!(hold.claim(vec![row("1", "a")]).0.len(), 1);
        drop(hold);
        assert_eq!(claims.held(), 0);
    }

    #[test]
    fn a_panicking_holder_releases_its_claims() {
        let claims = Claims::default();
        let mut receiver = claims.take_reoffers().expect("take");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut hold = claims.hold();
            hold.claim(vec![row("1", "a")]);
            let mut loser = claims.hold();
            loser.claim(vec![row("1", "a")]);
            panic!("mid-call");
        }));
        assert!(result.is_err());
        assert_eq!(claims.held(), 0);
        assert_eq!(drained(&mut receiver), ["1"]);
    }
}
