//! Per-channel bursts waiting for the debounce to run out (v4 `Burst` and its
//! timer task), as pure state over tokio instants: every added or edited
//! message pushes the channel's deadline out again.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::extract::backlog::BacklogEntry;

#[derive(Clone, Debug)]
struct Pending {
    messages: Vec<BacklogEntry>,
    deadline: Instant,
}

#[derive(Clone, Debug, Default)]
pub struct Bursts {
    pending: HashMap<String, Pending>,
}

impl Bursts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Add a message (once) and restart its channel's debounce.
    pub fn add(&mut self, message: BacklogEntry, now: Instant, debounce: Duration) {
        let pending = self
            .pending
            .entry(message.channel_id.clone())
            .or_insert_with(|| Pending {
                messages: Vec::new(),
                deadline: now,
            });
        if !pending
            .messages
            .iter()
            .any(|known| known.message_id == message.message_id)
        {
            pending.messages.push(message);
        }
        pending.deadline = now + debounce;
    }

    /// The channel's buffered messages, now (an urgent flush).
    pub fn take(&mut self, channel_id: &str) -> Option<Vec<BacklogEntry>> {
        self.pending
            .remove(channel_id)
            .map(|pending| pending.messages)
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|pending| pending.deadline).min()
    }

    /// Every channel whose debounce has run out, earliest first.
    pub fn due(&mut self, now: Instant) -> Vec<(String, Vec<BacklogEntry>)> {
        let mut due: Vec<(Instant, String)> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.deadline <= now)
            .map(|(channel, pending)| (pending.deadline, channel.clone()))
            .collect();
        due.sort();
        due.into_iter()
            .filter_map(|(_, channel)| {
                let messages = self.take(&channel)?;
                Some((channel, messages))
            })
            .collect()
    }

    /// Forget a deleted message; an emptied burst is dropped.
    pub fn forget(&mut self, message_id: &str) {
        self.pending.retain(|_, pending| {
            pending
                .messages
                .retain(|message| message.message_id != message_id);
            !pending.messages.is_empty()
        });
    }

    /// Everything still buffered (shutdown).
    pub fn drain(&mut self) -> Vec<(String, Vec<BacklogEntry>)> {
        self.pending
            .drain()
            .map(|(channel, pending)| (channel, pending.messages))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;

    const DEBOUNCE: Duration = Duration::from_secs(90);

    fn message(channel: &str, id: &str) -> BacklogEntry {
        BacklogEntry {
            channel_id: channel.into(),
            message_id: id.into(),
            created_at: DateTime::from_timestamp(1_790_000_000, 0).expect("instant"),
        }
    }

    // Instants are plain values here; no runtime or paused clock is needed.
    #[test]
    fn each_message_pushes_the_deadline_out() {
        let mut bursts = Bursts::new();
        let start = Instant::now();
        bursts.add(message("a", "1"), start, DEBOUNCE);
        let later = start + Duration::from_secs(60);
        bursts.add(message("a", "2"), later, DEBOUNCE);
        bursts.add(message("a", "2"), later, DEBOUNCE);
        assert_eq!(bursts.next_deadline(), Some(later + DEBOUNCE));
        assert!(bursts.due(start + DEBOUNCE).is_empty());
        let due = bursts.due(later + DEBOUNCE);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].1.len(), 2);
        assert!(bursts.is_empty());
    }

    #[test]
    fn a_deleted_message_leaves_its_burst() {
        let mut bursts = Bursts::new();
        let now = Instant::now();
        bursts.add(message("a", "1"), now, DEBOUNCE);
        bursts.add(message("b", "2"), now, DEBOUNCE);
        bursts.forget("1");
        assert!(bursts.take("a").is_none());
        assert_eq!(bursts.take("b").map(|m| m.len()), Some(1));
    }
}
