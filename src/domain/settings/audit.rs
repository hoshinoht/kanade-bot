//! Saved Config sections as History lists them: append-only records beside
//! the settings rows, never part of the schedule's hash chain and never
//! revertible. Every settings row is a structured value or admin-written
//! text, never a secret, so before/after row text is stored as is.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::{RuntimeSettings, Section, codec};
use crate::domain::history::{Actor, Surface};

/// One stored row's text before and after a save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDiff {
    pub from: String,
    pub to: String,
}

/// One effective section save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsChange {
    /// Store-assigned and increasing; ignored when appending.
    pub id: u64,
    pub at: DateTime<Utc>,
    pub actor: Actor,
    pub surface: Surface,
    /// The saved section (`persona`, `models`, …).
    pub section: String,
    /// The settings revision the save published (counted per process).
    pub revision: u64,
    /// Every stored row that differs; never empty.
    pub values: BTreeMap<String, RowDiff>,
}

/// Newest first; `from` inclusive, `until` exclusive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsChangeQuery {
    pub actor: Option<Actor>,
    pub from: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

impl SettingsChangeQuery {
    pub fn matches(&self, change: &SettingsChange) -> bool {
        self.actor
            .as_ref()
            .is_none_or(|actor| &change.actor == actor)
            && self.from.is_none_or(|from| change.at >= from)
            && self.until.is_none_or(|until| change.at < until)
    }
}

fn rows(settings: &RuntimeSettings) -> BTreeMap<&'static str, String> {
    [
        Section::Pings(settings.pings.clone()),
        Section::Watching(settings.watching.clone()),
        Section::Chatbot(settings.chatbot.clone()),
        Section::Notifications(settings.notifications),
        Section::SelfService(settings.self_service),
        Section::Persona(settings.persona.clone()),
        Section::Models(settings.models.clone()),
        Section::RunLengths(settings.run_lengths.clone()),
        Section::Profanity(settings.profanity.clone()),
        Section::Schedule(settings.schedule),
        Section::Posting(settings.posting.clone()),
    ]
    .iter()
    .flat_map(codec::encode)
    .collect()
}

/// Every stored row whose text differs between `before` and `after`.
pub fn diff_rows(before: &RuntimeSettings, after: &RuntimeSettings) -> BTreeMap<String, RowDiff> {
    let (before, after) = (rows(before), rows(after));
    after
        .into_iter()
        .filter(|(key, value)| before.get(key) != Some(value))
        .map(|(key, to)| {
            let from = before.get(key).cloned().unwrap_or_default();
            (key.to_owned(), RowDiff { from, to })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_rows_are_listed_with_both_texts() {
        let before = RuntimeSettings::default();
        assert!(diff_rows(&before, &before).is_empty());
        let mut after = before.clone();
        after.notifications.quiet_mode = !before.notifications.quiet_mode;
        let diff = diff_rows(&before, &after);
        assert_eq!(diff.len(), 1);
        let row = &diff["quiet_mode"];
        assert_ne!(row.from, row.to);
    }

    #[test]
    fn queries_filter_by_actor_and_half_open_window() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let change = SettingsChange {
            id: 1,
            at: at("2026-09-29T04:00:00Z"),
            actor: Actor::admin("token"),
            surface: Surface::AdminPortal,
            section: "pings".into(),
            revision: 1,
            values: BTreeMap::new(),
        };
        let mut query = SettingsChangeQuery::default();
        assert!(query.matches(&change));
        query.from = Some(change.at);
        assert!(query.matches(&change), "from is inclusive");
        query.until = Some(change.at);
        assert!(!query.matches(&change), "until is exclusive");
        let other = SettingsChangeQuery {
            actor: Some(Actor::admin("discord:1")),
            ..SettingsChangeQuery::default()
        };
        assert!(!other.matches(&change));
    }
}
