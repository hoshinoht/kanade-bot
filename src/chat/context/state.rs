//! Per-channel history, the last card's focus, re-anchorable exchanges and
//! the replied-author cache (v4 `ChatPilot._history`/`_focus`/`_anchors`/
//! `_replied`).

use std::collections::{BTreeSet, HashMap, VecDeque};

use super::{ANCHOR_CACHE, ChatTurn, HISTORY_EXCHANGES, REFERENCE_CACHE};
use crate::domain::pytext::strip;

#[derive(Clone, Debug)]
struct Focus {
    card: String,
    at: f64,
}

#[derive(Clone, Debug)]
struct Anchor {
    channel_id: String,
    question: ChatTurn,
    answer: ChatTurn,
}

/// A card as the focus line names it: `summary — party`, or the summary.
pub fn card_focus(summary: &str, party: &[String]) -> String {
    let summary = strip(summary);
    if summary.is_empty() || party.is_empty() {
        return summary.to_owned();
    }
    format!("{summary} — {}", party.join(", "))
}

/// Everything the pilot remembers between questions.
#[derive(Clone, Debug)]
pub struct Conversations {
    /// `CHAT_PILOT_HISTORY_TTL_S`.
    ttl: f64,
    history: HashMap<String, VecDeque<ChatTurn>>,
    focus: HashMap<String, Focus>,
    /// Insertion-ordered, oldest first.
    anchors: Vec<(String, Anchor)>,
    replied: Vec<(String, Option<String>)>,
    /// Message ids of withheld turns, oldest first, so a reply chain or a
    /// later remember never brings their text back.
    withheld: VecDeque<String>,
}

impl Conversations {
    pub fn new(ttl_seconds: f64) -> Self {
        Self {
            ttl: ttl_seconds,
            history: HashMap::new(),
            focus: HashMap::new(),
            anchors: Vec::new(),
            replied: Vec::new(),
            withheld: VecDeque::new(),
        }
    }

    /// Withhold a message from every later context: history, reply chains
    /// and anchors (bounded like the reference cache).
    pub fn withhold(&mut self, message_id: &str) {
        if message_id.is_empty() || self.is_withheld(message_id) {
            return;
        }
        if self.withheld.len() >= REFERENCE_CACHE {
            self.withheld.pop_front();
        }
        self.withheld.push_back(message_id.to_owned());
        for turns in self.history.values_mut() {
            for turn in turns.iter_mut() {
                if turn.message_id.as_deref() == Some(message_id) {
                    turn.withheld = true;
                }
            }
        }
        self.anchors.retain(|(id, anchor)| {
            id != message_id
                && anchor.question.message_id.as_deref() != Some(message_id)
                && anchor.answer.message_id.as_deref() != Some(message_id)
        });
    }

    pub fn is_withheld(&self, message_id: &str) -> bool {
        self.withheld.iter().any(|id| id == message_id)
    }

    /// The channel's live history, oldest first, after dropping expired turns.
    pub fn history(&mut self, channel_id: &str, now: f64) -> Vec<ChatTurn> {
        let cutoff = now - self.ttl;
        let turns = self.history.entry(channel_id.to_owned()).or_default();
        while turns
            .front()
            .is_some_and(|turn| turn.at.is_some_and(|at| at <= cutoff))
        {
            turns.pop_front();
        }
        turns.iter().cloned().collect()
    }

    /// Append a turn (stamped `now` unless it has a stamp); returns the
    /// channel's history length.
    pub fn remember(&mut self, channel_id: &str, mut turn: ChatTurn, now: f64) -> usize {
        turn.at.get_or_insert(now);
        if turn
            .message_id
            .as_deref()
            .is_some_and(|id| self.is_withheld(id))
        {
            turn.withheld = true;
        }
        self.history(channel_id, now);
        let turns = self.history.entry(channel_id.to_owned()).or_default();
        turns.push_back(turn);
        while turns.len() > HISTORY_EXCHANGES * 2 {
            turns.pop_front();
        }
        turns.len()
    }

    /// Clear one channel, or everything.
    pub fn forget(&mut self, channel_id: Option<&str>) {
        let Some(key) = channel_id else {
            self.history.clear();
            self.focus.clear();
            self.anchors.clear();
            return;
        };
        self.history.remove(key);
        self.focus.remove(key);
        self.anchors.retain(|(_, anchor)| anchor.channel_id != key);
    }

    /// Record the channel's most recently posted card (see [`card_focus`]).
    pub fn note_card(&mut self, channel_id: &str, card: &str, now: f64) {
        if !card.is_empty() {
            self.focus.insert(
                channel_id.to_owned(),
                Focus {
                    card: card.to_owned(),
                    at: now,
                },
            );
        }
    }

    /// The channel's current card, or `""` once it is as old as the history TTL.
    pub fn focus(&mut self, channel_id: &str, now: f64) -> String {
        match self.focus.get(channel_id) {
            None => String::new(),
            Some(entry) if now - entry.at >= self.ttl => {
                self.focus.remove(channel_id);
                String::new()
            }
            Some(entry) => entry.card.clone(),
        }
    }

    /// Keep an answered exchange keyed by the bot's reply message.
    pub fn anchor(
        &mut self,
        message_id: Option<&str>,
        channel_id: &str,
        question: ChatTurn,
        answer: ChatTurn,
    ) {
        let Some(key) = message_id.filter(|id| !id.is_empty()) else {
            return;
        };
        // A withheld exchange is never re-anchored.
        if question.withheld || answer.withheld || self.is_withheld(key) {
            return;
        }
        if self.anchors.len() >= ANCHOR_CACHE {
            self.anchors.remove(0);
        }
        let anchor = Anchor {
            channel_id: channel_id.to_owned(),
            question,
            answer,
        };
        // A re-anchored key keeps its place, as a Python dict does.
        match self.anchors.iter_mut().find(|(id, _)| id == key) {
            Some(slot) => slot.1 = anchor,
            None => self.anchors.push((key.to_owned(), anchor)),
        }
    }

    /// An anchored exchange a reply points at, minus turns already in the prompt.
    pub fn reanchored(&self, replied: Option<&str>, seen: &BTreeSet<String>) -> Vec<ChatTurn> {
        let Some(anchor) = replied.and_then(|replied| {
            self.anchors
                .iter()
                .find(|(id, _)| id == replied)
                .map(|(_, anchor)| anchor)
        }) else {
            return Vec::new();
        };
        let seen_id =
            |turn: &ChatTurn| turn.message_id.as_ref().is_some_and(|id| seen.contains(id));
        if seen_id(&anchor.answer) {
            return Vec::new();
        }
        [&anchor.question, &anchor.answer]
            .into_iter()
            .filter(|turn| !seen_id(turn))
            .cloned()
            .collect()
    }

    /// Remember who wrote a replied-to message (`None`: unknown).
    pub fn remember_reference(&mut self, message_id: &str, author_id: Option<String>) {
        if self.replied.len() >= REFERENCE_CACHE {
            self.replied.remove(0);
        }
        match self.replied.iter_mut().find(|(id, _)| id == message_id) {
            Some(slot) => slot.1 = author_id,
            None => self.replied.push((message_id.to_owned(), author_id)),
        }
    }

    /// A remembered replied-to author: `Some(None)` when looked up and unknown.
    pub fn replied_author(&self, message_id: &str) -> Option<Option<String>> {
        self.replied
            .iter()
            .find(|(id, _)| id == message_id)
            .map(|(_, author)| author.clone())
    }
}
