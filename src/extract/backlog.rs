//! The extraction backlog: late messages (RESUME replays, backfill on start,
//! turned-away bursts) wait here, deduplicated by message id and bounded, and
//! leave one burst at a time at the pipeline's fixed drain rate. Pure state;
//! the driver owns the timing.

use std::collections::{HashSet, VecDeque};

use chrono::{DateTime, Utc};

use super::window::{BURST_GAP, MAX_BURST_MESSAGES, group_bursts};

/// One queued message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BacklogEntry {
    pub channel_id: String,
    pub message_id: String,
    pub created_at: DateTime<Utc>,
}

/// Oldest-first queue with at most `capacity` entries and no repeated id.
#[derive(Clone, Debug)]
pub struct Backlog {
    capacity: usize,
    entries: VecDeque<BacklogEntry>,
    ids: HashSet<String>,
}

impl Backlog {
    /// A capacity of 0 is treated as 1.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: VecDeque::new(),
            ids: HashSet::new(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, message_id: &str) -> bool {
        self.ids.contains(message_id)
    }

    /// Queue at the back unless already queued; returns the oldest entries
    /// dropped to stay within capacity (the caller audits them).
    pub fn push(&mut self, entry: BacklogEntry) -> Vec<BacklogEntry> {
        if self.ids.insert(entry.message_id.clone()) {
            self.entries.push_back(entry);
        }
        self.trim()
    }

    /// Put a burst that could not run back at the front, in its order, so it
    /// is next; returns what capacity forced out.
    pub fn requeue(&mut self, entries: Vec<BacklogEntry>) -> Vec<BacklogEntry> {
        for entry in entries.into_iter().rev() {
            if self.ids.insert(entry.message_id.clone()) {
                self.entries.push_front(entry);
            }
        }
        self.trim()
    }

    /// Forget a deleted message; `true` when it was queued.
    pub fn remove(&mut self, message_id: &str) -> bool {
        if !self.ids.remove(message_id) {
            return false;
        }
        self.entries.retain(|entry| entry.message_id != message_id);
        true
    }

    /// The next burst: the oldest entry's channel, that channel's queued
    /// messages in time order up to the first quiet gap (v4 `BURST_GAP`) and
    /// at most [`MAX_BURST_MESSAGES`].
    pub fn next_burst(&mut self) -> Option<(String, Vec<BacklogEntry>)> {
        let channel = self.entries.front()?.channel_id.clone();
        let mut queued: Vec<BacklogEntry> = self
            .entries
            .iter()
            .filter(|entry| entry.channel_id == channel)
            .cloned()
            .collect();
        queued.sort_by(|a, b| (a.created_at, &a.message_id).cmp(&(b.created_at, &b.message_id)));
        let mut burst = group_bursts(&queued, BURST_GAP, |entry| entry.created_at)
            .into_iter()
            .next()
            .unwrap_or_default();
        burst.truncate(MAX_BURST_MESSAGES);
        for entry in &burst {
            self.remove(&entry.message_id);
        }
        Some((channel, burst))
    }

    fn trim(&mut self) -> Vec<BacklogEntry> {
        let mut dropped = Vec::new();
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.entries.pop_front() else {
                break;
            };
            self.ids.remove(&oldest.message_id);
            dropped.push(oldest);
        }
        dropped
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn entry(channel: &str, id: &str, minute: i64) -> BacklogEntry {
        let base = DateTime::from_timestamp(1_790_000_000, 0).expect("instant");
        BacklogEntry {
            channel_id: channel.into(),
            message_id: id.into(),
            created_at: base + TimeDelta::minutes(minute),
        }
    }

    fn ids(entries: &[BacklogEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| entry.message_id.as_str())
            .collect()
    }

    #[test]
    fn a_repeated_id_is_queued_once() {
        let mut backlog = Backlog::new(10);
        backlog.push(entry("c", "1", 0));
        backlog.push(entry("c", "1", 0));
        assert_eq!(backlog.len(), 1);
    }

    #[test]
    fn a_full_backlog_drops_its_oldest_entries() {
        let mut backlog = Backlog::new(2);
        assert!(backlog.push(entry("c", "1", 0)).is_empty());
        backlog.push(entry("c", "2", 1));
        let dropped = backlog.push(entry("c", "3", 2));
        assert_eq!(ids(&dropped), ["1"]);
        assert!(!backlog.contains("1"));
        // A dropped id may come back later.
        backlog.push(entry("c", "1", 0));
        assert!(backlog.contains("1"));
    }

    #[test]
    fn a_burst_is_the_oldest_channel_up_to_its_first_quiet_gap() {
        let mut backlog = Backlog::new(100);
        backlog.push(entry("a", "a2", 10));
        backlog.push(entry("b", "b1", 0));
        backlog.push(entry("a", "a1", 5));
        backlog.push(entry("a", "a3", 5 + 60 * 4));
        let (channel, burst) = backlog.next_burst().expect("burst");
        assert_eq!(channel, "a");
        assert_eq!(ids(&burst), ["a1", "a2"]);
        let (channel, _) = backlog.next_burst().expect("burst");
        assert_eq!(channel, "b");
        let (_, burst) = backlog.next_burst().expect("burst");
        assert_eq!(ids(&burst), ["a3"]);
        assert!(backlog.next_burst().is_none());
    }

    #[test]
    fn a_burst_is_capped_and_a_requeued_one_goes_first() {
        let mut backlog = Backlog::new(100);
        for n in 0..20 {
            backlog.push(entry("a", &format!("m{n:02}"), n));
        }
        let (_, burst) = backlog.next_burst().expect("burst");
        assert_eq!(burst.len(), MAX_BURST_MESSAGES);
        backlog.push(entry("b", "b1", 0));
        backlog.requeue(burst.clone());
        let (channel, again) = backlog.next_burst().expect("burst");
        assert_eq!(channel, "a");
        assert_eq!(again, burst);
    }

    #[test]
    fn a_deleted_message_leaves_the_backlog() {
        let mut backlog = Backlog::new(10);
        backlog.push(entry("a", "1", 0));
        assert!(backlog.remove("1"));
        assert!(!backlog.remove("1"));
        assert!(backlog.is_empty());
    }
}
