//! A proposed schedule change as the extractor or chatbot hands it over
//! (v4's `amendments` row), and the target facts stored with its proposal.

use std::collections::BTreeSet;

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use serde_json::{Value, json};

use crate::domain::schedule::RsvpState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChangeKind {
    Move,
    Add,
    Cancel,
    Split,
    Otot,
    Sub,
    Rsvp,
    Fix,
}

impl ChangeKind {
    pub const ALL: [Self; 8] = [
        Self::Move,
        Self::Add,
        Self::Cancel,
        Self::Split,
        Self::Otot,
        Self::Sub,
        Self::Rsvp,
        Self::Fix,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Add => "add",
            Self::Cancel => "cancel",
            Self::Split => "split",
            Self::Otot => "otot",
            Self::Sub => "sub",
            Self::Rsvp => "rsvp",
            Self::Fix => "fix",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// What a change needs beyond the common fields. A kind with the wrong (or
/// no) payload reads it as empty, as v4 read a missing payload key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Payload {
    #[default]
    None,
    Sub {
        remove: Vec<String>,
        add: Vec<String>,
    },
    /// `bosses: None` falls back to the change's bosses (v4
    /// `payload.get("bosses", amendment["bosses"])`).
    Split {
        bosses: Option<Vec<String>>,
        participants: Vec<String>,
    },
    /// A new weekly timing's slot.
    Fix {
        weekday: Option<Weekday>,
        time: Option<NaiveTime>,
    },
    /// v4 `FIX_EDIT`: change a timing in place.
    FixEdit {
        fixed_run_id: Option<String>,
        weekday: Option<Weekday>,
        time: Option<NaiveTime>,
        participants: Vec<String>,
    },
    /// v4 `FIX_REMOVE`: retire a timing.
    FixRemove { fixed_run_id: Option<String> },
}

/// One proposed change. `channel_id` is where its card is posted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedChange {
    pub kind: ChangeKind,
    pub run_id: Option<String>,
    pub channel_id: Option<String>,
    pub bosses: Vec<String>,
    pub participants: Vec<String>,
    pub new_datetime: Option<DateTime<Utc>>,
    pub rsvp: Option<RsvpState>,
    pub payload: Payload,
}

impl ProposedChange {
    /// A change of `kind` with every other field empty.
    pub fn new(kind: ChangeKind) -> Self {
        Self {
            kind,
            run_id: None,
            channel_id: None,
            bosses: Vec::new(),
            participants: Vec::new(),
            new_datetime: None,
            rsvp: None,
            payload: Payload::None,
        }
    }
}

/// What a proposal targets, stored as its draft `subject` (sorted-key
/// JSON): enough to supersede siblings and check who may approve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalSubject {
    pub kind: ChangeKind,
    pub run_id: Option<String>,
    /// The timing a weekly edit or removal names.
    pub fixed_run_id: Option<String>,
    pub channel_id: Option<String>,
    pub bosses: Vec<String>,
    /// Who the change names (v4 `may_commit` for changes without a run).
    pub named: Vec<String>,
}

impl ProposalSubject {
    pub fn encode(&self) -> String {
        json!({
            "kind": self.kind.as_str(),
            "run_id": self.run_id,
            "fixed_run_id": self.fixed_run_id,
            "channel_id": self.channel_id,
            "bosses": self.bosses,
            "named": self.named,
        })
        .to_string()
    }

    pub fn parse(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        let optional = |name: &str| match value.get(name)? {
            Value::Null => Some(None),
            Value::String(text) => Some(Some(text.clone())),
            _ => None,
        };
        let list = |name: &str| -> Option<Vec<String>> {
            value
                .get(name)?
                .as_array()?
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect()
        };
        Some(Self {
            kind: ChangeKind::parse(value.get("kind")?.as_str()?)?,
            run_id: optional("run_id")?,
            fixed_run_id: optional("fixed_run_id")?,
            channel_id: optional("channel_id")?,
            bosses: list("bosses")?,
            named: list("named")?,
        })
    }

    /// The store supersede key: the target run in this channel, else the
    /// boss set a run-less change would create in this channel (v4's two
    /// supersede keys, channel-scoped).
    pub fn supersede_key(&self) -> Option<String> {
        let channel = self.channel_id.as_deref().unwrap_or("-");
        if let Some(run) = &self.run_id {
            return Some(format!("run:{run}#{channel}"));
        }
        if self.bosses.is_empty() {
            return None;
        }
        let bosses: BTreeSet<&str> = self.bosses.iter().map(String::as_str).collect();
        Some(format!(
            "bosses:{channel}#{}",
            bosses.into_iter().collect::<Vec<_>>().join("\u{1f}")
        ))
    }

    /// Same boss set, ignoring order (v4 `proposed_for_bosses`).
    pub fn same_bosses(&self, bosses: &[String]) -> bool {
        let mine: BTreeSet<&String> = self.bosses.iter().collect();
        mine == bosses.iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_round_trip_and_key_by_run_else_boss_set() {
        let subject = ProposalSubject {
            kind: ChangeKind::Add,
            run_id: None,
            fixed_run_id: None,
            channel_id: Some("900".into()),
            bosses: vec!["NKalos".into(), "NLimbo".into()],
            named: vec!["4".into()],
        };
        assert_eq!(
            ProposalSubject::parse(&subject.encode()),
            Some(subject.clone())
        );
        let swapped = ProposalSubject {
            bosses: vec!["NLimbo".into(), "NKalos".into()],
            ..subject.clone()
        };
        assert_eq!(subject.supersede_key(), swapped.supersede_key());
        let run = ProposalSubject {
            run_id: Some("r-1".into()),
            ..subject.clone()
        };
        assert_eq!(run.supersede_key().as_deref(), Some("run:r-1#900"));
        let nothing = ProposalSubject {
            bosses: Vec::new(),
            ..subject
        };
        assert_eq!(nothing.supersede_key(), None);
        assert_eq!(ProposalSubject::parse("{}"), None);
    }
}
