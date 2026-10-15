//! The parts of the scouter's `GET /v1/bosses` answers kanade reads;
//! every other field is ignored. Boss keys are the sidecar's
//! `<difficulty>_<boss>` card keys (`hard_limbo`), mapped to catalog tokens
//! by [`super::keys`] on the way out. Results are stored in the
//! character-data file in kanade's own shape (`domain::chardata`), never
//! as the sidecar's JSON.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::scouter_keys;
use crate::domain::chardata::{StoredCard, StoredCombined, StoredMember};

/// One character: the `ign=` answer, or one of a batch's `members`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Member {
    pub ign: String,
    /// The preset the result was read with (`00010`).
    pub preset: String,
    /// The character's critical rate is under 100% and the scouter's
    /// manual Input flow raised it to exactly 100 for this result: a "what
    /// if crit were 100%" estimate, labelled as such wherever it shows.
    #[serde(default)]
    pub critical_rate_assumed: bool,
    /// The character's own critical rate, with an assumed result.
    #[serde(default)]
    pub critical_rate_actual: Option<f64>,
    /// `manual_input` with an assumed result; absent for the normal page.
    #[serde(default)]
    pub source: Option<String>,
    pub bosses: BTreeMap<String, Card>,
}

impl Member {
    /// A result from the manual Input flow: either marker labels it, so a
    /// "what if" result never shows as the normal benchmark.
    pub fn crit_assumed(&self) -> bool {
        self.critical_rate_assumed || self.source.as_deref() == Some("manual_input")
    }

    /// The character's own share for catalog `token` (its card's
    /// `effective_percentage`, never the card's whole-party figure).
    pub fn effective(&self, token: &str) -> Option<f64> {
        scouter_keys(token)
            .find_map(|key| self.bosses.get(key))
            .and_then(|card| card.effective_percentage)
    }
}

/// One boss card. Unknown values stay `null`; a missing card is never 0%.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Card {
    pub percentage: f64,
    pub rating: String,
    pub party: bool,
    pub effective_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
}

/// The `igns=` answer (its `members` repeat the `ign=` answers kanade
/// already holds).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Batch {
    pub combined: BTreeMap<String, Combined>,
}

/// The party's additive estimate for one boss key.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Combined {
    pub combined_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub technically_possible: Option<bool>,
    pub complete: bool,
    /// IGN → `missing_boss` / `unknown_benchmark_players`.
    #[serde(default)]
    pub issues: BTreeMap<String, String>,
}

/// `{"error": {"code", "message", "ign"?}}`; the message is never kept.
#[derive(Deserialize)]
pub(super) struct ErrorBody {
    pub error: ErrorField,
}

#[derive(Deserialize)]
pub(super) struct ErrorField {
    pub code: String,
    #[serde(default)]
    pub ign: Option<String>,
}

impl From<&Member> for StoredMember {
    fn from(member: &Member) -> Self {
        Self {
            ign: member.ign.clone(),
            preset: member.preset.clone(),
            critical_rate_assumed: member.critical_rate_assumed,
            critical_rate_actual: member.critical_rate_actual,
            source: member.source.clone(),
            bosses: member
                .bosses
                .iter()
                .map(|(key, card)| {
                    let stored = StoredCard {
                        percentage: card.percentage,
                        rating: card.rating.clone(),
                        party: card.party,
                        effective_percentage: card.effective_percentage,
                        estimated_minutes: card.estimated_minutes,
                    };
                    (key.clone(), stored)
                })
                .collect(),
        }
    }
}

impl From<StoredMember> for Member {
    fn from(stored: StoredMember) -> Self {
        Self {
            ign: stored.ign,
            preset: stored.preset,
            critical_rate_assumed: stored.critical_rate_assumed,
            critical_rate_actual: stored.critical_rate_actual,
            source: stored.source,
            bosses: stored
                .bosses
                .into_iter()
                .map(|(key, card)| {
                    let card = Card {
                        percentage: card.percentage,
                        rating: card.rating,
                        party: card.party,
                        effective_percentage: card.effective_percentage,
                        estimated_minutes: card.estimated_minutes,
                    };
                    (key, card)
                })
                .collect(),
        }
    }
}

impl From<&Combined> for StoredCombined {
    fn from(total: &Combined) -> Self {
        Self {
            combined_percentage: total.combined_percentage,
            estimated_minutes: total.estimated_minutes,
            technically_possible: total.technically_possible,
            complete: total.complete,
            issues: total.issues.clone(),
        }
    }
}

impl From<StoredCombined> for Combined {
    fn from(stored: StoredCombined) -> Self {
        Self {
            combined_percentage: stored.combined_percentage,
            estimated_minutes: stored.estimated_minutes,
            technically_possible: stored.technically_possible,
            complete: stored.complete,
            issues: stored.issues,
        }
    }
}
