use std::{collections::BTreeMap, sync::Arc};

use tokio::{sync::Notify, time::Instant};

/// Queue classes, highest first; FIFO within a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Priority {
    Admin,
    /// A question already under way (for example requeued after losing its slot).
    ChatRound,
    ChatNew,
    Extraction,
    FollowUp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CallKind {
    Chat,
    Extraction,
    Rescan,
    FollowUp,
    /// Never queues: `try_acquire` only.
    Rewrite,
    /// Never queues: `try_acquire` only, and never on an external route.
    PreScreen,
}

impl CallKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Extraction => "extraction",
            Self::Rescan => "rescan",
            Self::FollowUp => "follow_up",
            Self::Rewrite => "rewrite",
            Self::PreScreen => "pre_screen",
        }
    }

    pub fn may_wait(self) -> bool {
        !matches!(self, Self::Rewrite | Self::PreScreen)
    }
}

pub(super) type QueueKey = (Priority, u64);

#[derive(Debug)]
pub(super) struct Entry {
    pub(super) kind: CallKind,
    pub(super) who: String,
    pub(super) since: Instant,
    wake: Arc<Notify>,
}

#[derive(Clone, Debug)]
pub(super) struct Holder {
    pub(super) kind: CallKind,
    pub(super) who: String,
    pub(super) since: Instant,
}

#[derive(Debug)]
pub(super) struct PermitPool {
    pub(super) total: u32,
    pub(super) in_use: u32,
    next: u64,
    pub(super) queue: BTreeMap<QueueKey, Entry>,
    pub(super) holders: BTreeMap<u64, Holder>,
}

impl PermitPool {
    pub(super) fn new(total: u32) -> Self {
        Self {
            total,
            in_use: 0,
            next: 0,
            queue: BTreeMap::new(),
            holders: BTreeMap::new(),
        }
    }

    fn sequence(&mut self) -> u64 {
        self.next += 1;
        self.next
    }

    pub(super) fn enqueue(
        &mut self,
        priority: Priority,
        kind: CallKind,
        who: String,
        now: Instant,
    ) -> (QueueKey, Arc<Notify>) {
        let key = (priority, self.sequence());
        let wake = Arc::new(Notify::new());
        self.queue.insert(
            key,
            Entry {
                kind,
                who,
                since: now,
                wake: wake.clone(),
            },
        );
        (key, wake)
    }

    pub(super) fn is_queued(&self, key: &QueueKey) -> bool {
        self.queue.contains_key(key)
    }

    pub(super) fn cancel(&mut self, key: &QueueKey) -> bool {
        self.queue.remove(key).is_some()
    }

    /// Hands free permits (up to `limit`) to the head of the queue; the holder id
    /// is the waiter's sequence number.
    pub(super) fn grant(&mut self, limit: u32, now: Instant) {
        while self.in_use < limit.min(self.total) {
            let Some(((_, id), entry)) = self.queue.pop_first() else {
                break;
            };
            self.take(id, entry.kind, entry.who, now);
            entry.wake.notify_one();
        }
    }

    /// Never jumps a queued waiter.
    pub(super) fn try_take(
        &mut self,
        limit: u32,
        kind: CallKind,
        who: String,
        now: Instant,
    ) -> Option<u64> {
        if !self.queue.is_empty() || self.in_use >= limit.min(self.total) {
            return None;
        }
        let id = self.sequence();
        self.take(id, kind, who, now);
        Some(id)
    }

    fn take(&mut self, id: u64, kind: CallKind, who: String, now: Instant) {
        self.in_use += 1;
        self.holders.insert(
            id,
            Holder {
                kind,
                who,
                since: now,
            },
        );
    }

    /// Makes every waiter recompute its wake-up time after a breaker transition.
    pub(super) fn wake_all(&self) {
        for entry in self.queue.values() {
            entry.wake.notify_one();
        }
    }

    pub(super) fn release(&mut self, id: u64) {
        if self.holders.remove(&id).is_some() {
            self.in_use -= 1;
        }
    }
}
