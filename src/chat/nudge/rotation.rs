//! Per-channel seed rotation: a uniform pick among the pool's lines that the
//! channel has not seen recently, so a line never repeats back to back.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

use crate::infrastructure::llm::governor::Random;

/// Lines remembered per channel; short pools exclude fewer so a choice remains.
pub const RECENT_PER_CHANNEL: usize = 3;
/// Channels remembered; the least recently nudged one is forgotten first.
pub const MAX_CHANNELS: usize = 1024;

#[derive(Default)]
struct Recent {
    /// Pick counter at this channel's last use, for LRU eviction.
    used: u64,
    lines: VecDeque<String>,
}

#[derive(Default)]
struct State {
    ticks: u64,
    channels: HashMap<String, Recent>,
}

pub struct SeedRotation {
    random: Arc<dyn Random>,
    state: Mutex<State>,
}

impl std::fmt::Debug for SeedRotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeedRotation").finish_non_exhaustive()
    }
}

impl SeedRotation {
    pub fn new(random: Arc<dyn Random>) -> Self {
        Self {
            random,
            state: Mutex::new(State::default()),
        }
    }

    /// Channels currently remembered (at most [`MAX_CHANNELS`]).
    pub fn channels(&self) -> usize {
        self.lock().channels.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Picks uniformly among `lines` not among the channel's last
    /// `min(RECENT_PER_CHANNEL, lines - 1)` picks and records the choice.
    /// Recent lines are matched by text, so switching pools stays fair.
    pub fn pick<'a>(&self, channel_id: &str, lines: &[&'a str]) -> Option<&'a str> {
        if lines.is_empty() {
            return None;
        }
        let mut state = self.lock();
        state.ticks += 1;
        let tick = state.ticks;
        if !state.channels.contains_key(channel_id) && state.channels.len() >= MAX_CHANNELS {
            // O(n) only when a new channel arrives at the cap.
            let oldest = state
                .channels
                .iter()
                .min_by_key(|(_, recent)| recent.used)
                .map(|(channel, _)| channel.clone());
            if let Some(oldest) = oldest {
                state.channels.remove(&oldest);
            }
        }
        let seen = state.channels.entry(channel_id.to_owned()).or_default();
        seen.used = tick;
        let window = RECENT_PER_CHANNEL.min(lines.len() - 1);
        let excluded: Vec<&str> = seen
            .lines
            .iter()
            .rev()
            .take(window)
            .map(String::as_str)
            .collect();
        let mut candidates: Vec<&'a str> = lines
            .iter()
            .copied()
            .filter(|line| !excluded.contains(line))
            .collect();
        if candidates.is_empty() {
            candidates = lines.to_vec();
        }
        let index = usize::try_from(self.random.next_u64() % candidates.len() as u64).unwrap_or(0);
        let chosen = candidates[index];
        seen.lines.push_back(chosen.to_owned());
        while seen.lines.len() > RECENT_PER_CHANNEL {
            seen.lines.pop_front();
        }
        Some(chosen)
    }
}
