//! Per-channel seed rotation: a uniform pick among the pool's lines that the
//! channel has not seen recently, so a line never repeats back to back.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, PoisonError},
};

use crate::infrastructure::llm::governor::Random;

/// Lines remembered per channel; short pools exclude fewer so a choice remains.
pub const RECENT_PER_CHANNEL: usize = 3;

pub struct SeedRotation {
    random: Arc<dyn Random>,
    /// Keyed by channel id; bounded per channel, and channels are the guild's.
    recent: Mutex<HashMap<String, VecDeque<String>>>,
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
            recent: Mutex::new(HashMap::new()),
        }
    }

    /// Picks uniformly among `lines` not among the channel's last
    /// `min(RECENT_PER_CHANNEL, lines - 1)` picks and records the choice.
    /// Recent lines are matched by text, so switching pools stays fair.
    pub fn pick<'a>(&self, channel_id: &str, lines: &[&'a str]) -> Option<&'a str> {
        if lines.is_empty() {
            return None;
        }
        let mut recent = self.recent.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = recent.entry(channel_id.to_owned()).or_default();
        let window = RECENT_PER_CHANNEL.min(lines.len() - 1);
        let excluded: Vec<&str> = seen.iter().rev().take(window).map(String::as_str).collect();
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
        seen.push_back(chosen.to_owned());
        while seen.len() > RECENT_PER_CHANNEL {
            seen.pop_front();
        }
        Some(chosen)
    }
}
