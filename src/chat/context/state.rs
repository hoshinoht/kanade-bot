//! Per-channel history, the last card's focus, re-anchorable exchanges and
//! the replied-author cache (v4 `ChatPilot._history`/`_focus`/`_anchors`/
//! `_replied`).

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

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
    question: Option<ChatTurn>,
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
    /// later remember never brings their text back; indexed for lookup.
    withheld: VecDeque<String>,
    withheld_index: HashSet<String>,
    /// Message ids dropped from every later context (not even a
    /// placeholder): profanity-deflected exchanges. Same bound as `withheld`.
    excluded: VecDeque<String>,
    excluded_index: HashSet<String>,
}

/// Withheld ids kept. A reply can reach any old message, so eviction is by
/// count, not the history TTL; blocked questions are rare, and 4096 ids
/// (tens of KB) outlast the chat-log retention any realistic guild fills,
/// with the reload at startup restoring the newest of them.
pub const WITHHELD_CACHE: usize = 4096;

impl Conversations {
    pub fn new(ttl_seconds: f64) -> Self {
        Self {
            ttl: ttl_seconds,
            history: HashMap::new(),
            focus: HashMap::new(),
            anchors: Vec::new(),
            replied: Vec::new(),
            withheld: VecDeque::new(),
            withheld_index: HashSet::new(),
            excluded: VecDeque::new(),
            excluded_index: HashSet::new(),
        }
    }

    /// Drop a message from every later context entirely: its history turn
    /// and any anchor holding it go, and `remember`, reply chains and
    /// re-anchoring skip it (at most [`WITHHELD_CACHE`], oldest evicted).
    pub fn exclude(&mut self, message_id: &str) {
        if message_id.is_empty() || self.is_excluded(message_id) {
            return;
        }
        if self.excluded.len() >= WITHHELD_CACHE
            && let Some(oldest) = self.excluded.pop_front()
        {
            self.excluded_index.remove(&oldest);
        }
        self.excluded.push_back(message_id.to_owned());
        self.excluded_index.insert(message_id.to_owned());
        let id = Some(message_id);
        for turns in self.history.values_mut() {
            turns.retain(|turn| turn.message_id.as_deref() != id);
        }
        self.drop_anchors_with(message_id);
    }

    pub fn is_excluded(&self, message_id: &str) -> bool {
        self.excluded_index.contains(message_id)
    }

    /// Withhold a message from every later context: history, reply chains
    /// and anchors (at most [`WITHHELD_CACHE`], oldest evicted first).
    pub fn withhold(&mut self, message_id: &str) {
        if message_id.is_empty() || self.is_withheld(message_id) {
            return;
        }
        if self.withheld.len() >= WITHHELD_CACHE
            && let Some(oldest) = self.withheld.pop_front()
        {
            self.withheld_index.remove(&oldest);
        }
        self.withheld.push_back(message_id.to_owned());
        self.withheld_index.insert(message_id.to_owned());
        for turns in self.history.values_mut() {
            for turn in turns.iter_mut() {
                if turn.message_id.as_deref() == Some(message_id) {
                    turn.withheld = true;
                }
            }
        }
        self.drop_anchors_with(message_id);
    }

    /// Drop the anchors keyed by `message_id` or holding it as their question
    /// or answer, in any channel; every other anchor stays.
    fn drop_anchors_with(&mut self, message_id: &str) {
        let id = Some(message_id);
        self.anchors.retain(|(key, anchor)| {
            key != message_id
                && anchor.answer.message_id.as_deref() != id
                && anchor
                    .question
                    .as_ref()
                    .is_none_or(|question| question.message_id.as_deref() != id)
        });
    }

    pub fn is_withheld(&self, message_id: &str) -> bool {
        self.withheld_index.contains(message_id)
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
            .is_some_and(|id| self.is_excluded(id))
        {
            return self.history(channel_id, now).len();
        }
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
        // A withheld or excluded exchange is never re-anchored.
        if question.withheld || answer.withheld || self.is_withheld(key) || self.is_excluded(key) {
            return;
        }
        if self.anchors.len() >= ANCHOR_CACHE {
            self.anchors.remove(0);
        }
        let anchor = Anchor {
            channel_id: channel_id.to_owned(),
            question: Some(question),
            answer,
        };
        // A re-anchored key keeps its place, as a Python dict does.
        match self.anchors.iter_mut().find(|(id, _)| id == key) {
            Some(slot) => slot.1 = anchor,
            None => self.anchors.push((key.to_owned(), anchor)),
        }
    }

    /// Anchor a visible assistant-only turn. Rejection follow-ups have no
    /// member message to retain: the synthetic prompt must not enter history.
    pub fn anchor_assistant(&mut self, message_id: &str, channel_id: &str, answer: ChatTurn) {
        if message_id.is_empty()
            || answer.withheld
            || self.is_withheld(message_id)
            || self.is_excluded(message_id)
        {
            return;
        }
        if self.anchors.len() >= ANCHOR_CACHE {
            self.anchors.remove(0);
        }
        let anchor = Anchor {
            channel_id: channel_id.to_owned(),
            question: None,
            answer,
        };
        match self.anchors.iter_mut().find(|(id, _)| id == message_id) {
            Some(slot) => slot.1 = anchor,
            None => self.anchors.push((message_id.to_owned(), anchor)),
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
        anchor
            .question
            .iter()
            .chain(std::iter::once(&anchor.answer))
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::Conversations;
    use crate::chat::context::{ChatTurn, TurnRole};

    fn turn(role: TurnRole, id: &str) -> ChatTurn {
        ChatTurn::new(role, format!("text {id}"), Some(id.to_owned()))
    }

    /// Anchors `q-<n>` → `a-<n>` keyed by `a-<n>` in `channel`.
    fn anchored(conversations: &mut Conversations, n: &str, channel: &str) {
        conversations.anchor(
            Some(&format!("a-{n}")),
            channel,
            turn(TurnRole::User, &format!("q-{n}")),
            turn(TurnRole::Assistant, &format!("a-{n}")),
        );
    }

    fn reachable(conversations: &Conversations, key: &str) -> bool {
        !conversations
            .reanchored(Some(key), &BTreeSet::new())
            .is_empty()
    }

    /// Regression: `withhold` once kept only anchors whose question was the
    /// withheld message, dropping every unrelated anchor in all channels.
    /// Withholding or excluding one message drops only the anchors tied to it.
    #[test]
    fn withholding_or_excluding_drops_only_the_anchors_tied_to_that_message() {
        for drop_by_question in [true, false] {
            for excluding in [false, true] {
                let mut conversations = Conversations::new(2700.0);
                for (n, channel) in [("1", "700"), ("2", "700"), ("3", "800")] {
                    anchored(&mut conversations, n, channel);
                }
                let target = if drop_by_question { "q-1" } else { "a-1" };
                if excluding {
                    conversations.exclude(target);
                } else {
                    conversations.withhold(target);
                }
                let case = format!("{target} excluding={excluding}");
                assert!(
                    !reachable(&conversations, "a-1"),
                    "{case}: tied anchor dropped"
                );
                assert!(
                    reachable(&conversations, "a-2"),
                    "{case}: same channel kept"
                );
                assert!(
                    reachable(&conversations, "a-3"),
                    "{case}: other channel kept"
                );
            }
        }
        let mut conversations = Conversations::new(2700.0);
        anchored(&mut conversations, "1", "700");
        conversations.anchor_assistant("a-4", "700", turn(TurnRole::Assistant, "a-4"));
        conversations.withhold("unrelated");
        assert!(reachable(&conversations, "a-1") && reachable(&conversations, "a-4"));
        conversations.withhold("a-4");
        assert!(
            reachable(&conversations, "a-1"),
            "an assistant-only anchor goes alone"
        );
        assert!(!reachable(&conversations, "a-4"));
    }
}
