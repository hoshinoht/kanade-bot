//! One answer at a time per channel, with a short fair queue (user decision
//! 2026-09-24; v4 dropped a second question with a busy reaction). Waiting
//! questions are served FIFO per channel, bounded per channel and
//! guild-wide, given up after a maximum wait, and cancellable when the
//! author deletes the message. Pure state; monotonic seconds passed in.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Queue bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrafficLimits {
    /// Questions waiting behind one channel's answer.
    pub per_channel: usize,
    /// Questions waiting across the guild.
    pub guild: usize,
    /// Seconds a question may wait before it is given up.
    pub max_wait_s: f64,
}

impl Default for TrafficLimits {
    fn default() -> Self {
        Self {
            per_channel: 3,
            guild: 10,
            max_wait_s: 120.0,
        }
    }
}

/// One waiting question.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub channel_id: String,
    pub message_id: String,
    pub member_id: String,
    /// When it was queued (monotonic).
    pub since: f64,
    /// When its allowance was spent (for a refund if it is dropped).
    pub spent_at: f64,
}

/// What happens to a question the gate accepted.
#[derive(Clone, Debug, PartialEq)]
pub enum Admission {
    /// Answer now; the channel is marked busy until [`Traffic::finish`].
    Answer,
    /// Queued; 1-based position in its channel.
    Queued { position: usize },
    /// The queue is full: give up now (busy reaction, refund).
    Busy,
}

/// The Limits page's view of the queue.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueView {
    pub answering: Vec<String>,
    pub waiting: Vec<Waiting>,
}

#[derive(Clone, Debug, Default)]
pub struct Traffic {
    limits: TrafficLimits,
    answering: BTreeSet<String>,
    queues: BTreeMap<String, VecDeque<Waiting>>,
}

impl Traffic {
    pub fn new(limits: TrafficLimits) -> Self {
        Self {
            limits,
            answering: BTreeSet::new(),
            queues: BTreeMap::new(),
        }
    }

    fn waiting(&self) -> usize {
        self.queues.values().map(VecDeque::len).sum()
    }

    /// Admit an accepted question.
    pub fn admit(
        &mut self,
        channel_id: &str,
        message_id: &str,
        member_id: &str,
        (now, spent_at): (f64, f64),
    ) -> Admission {
        if self.answering.insert(channel_id.to_owned()) {
            return Admission::Answer;
        }
        let here = self.queues.get(channel_id).map_or(0, VecDeque::len);
        if here >= self.limits.per_channel || self.waiting() >= self.limits.guild {
            return Admission::Busy;
        }
        let queue = self.queues.entry(channel_id.to_owned()).or_default();
        queue.push_back(Waiting {
            channel_id: channel_id.to_owned(),
            message_id: message_id.to_owned(),
            member_id: member_id.to_owned(),
            since: now,
            spent_at,
        });
        Admission::Queued {
            position: queue.len(),
        }
    }

    /// The channel's answer is done: the next waiting question to answer,
    /// if any (the channel stays busy for it).
    pub fn finish(&mut self, channel_id: &str) -> Option<Waiting> {
        let next = self
            .queues
            .get_mut(channel_id)
            .and_then(VecDeque::pop_front);
        if next.is_none() {
            self.answering.remove(channel_id);
            self.queues.remove(channel_id);
        }
        next
    }

    /// The author deleted a waiting question; returns it for a refund.
    pub fn cancel(&mut self, message_id: &str) -> Option<Waiting> {
        for queue in self.queues.values_mut() {
            if let Some(at) = queue.iter().position(|w| w.message_id == message_id) {
                return queue.remove(at);
            }
        }
        None
    }

    /// Give up every question that waited longer than the bound; returns
    /// them (busy reaction, refund).
    pub fn expire(&mut self, now: f64) -> Vec<Waiting> {
        let mut dropped = Vec::new();
        for queue in self.queues.values_mut() {
            while queue
                .front()
                .is_some_and(|w| now - w.since >= self.limits.max_wait_s)
            {
                dropped.extend(queue.pop_front());
            }
        }
        dropped
    }

    /// 1-based position of a waiting question in its channel.
    pub fn position(&self, message_id: &str) -> Option<usize> {
        self.queues.values().find_map(|queue| {
            queue
                .iter()
                .position(|w| w.message_id == message_id)
                .map(|at| at + 1)
        })
    }

    pub fn view(&self) -> QueueView {
        QueueView {
            answering: self.answering.iter().cloned().collect(),
            waiting: self.queues.values().flatten().cloned().collect(),
        }
    }
}
