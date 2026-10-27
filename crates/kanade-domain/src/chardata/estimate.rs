//! Scouter estimates kept in the character-data file: each character's
//! latest result and each party's latest total, a cache kept at most
//! [`ESTIMATE_TTL`]. The stored JSON is kanade's own shape (field names as
//! below), so a change to the sidecar's answers never reinterprets a row.
//! Never in the change history, backups or a log.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

use super::FUTURE_SKEW;
use crate::scheduler::StoreError;

/// The longest an estimate is kept, from its fetch.
pub const ESTIMATE_TTL: TimeDelta = TimeDelta::days(30);

/// Kept at `now`: younger than [`ESTIMATE_TTL`] and not stamped further
/// ahead than [`FUTURE_SKEW`].
pub fn estimate_kept(fetched_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now - ESTIMATE_TTL < fetched_at && fetched_at <= now + FUTURE_SKEW
}

/// One character's result, as the scouter answered `ign=`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredMember {
    pub ign: String,
    /// The preset the result was read with.
    pub preset: String,
    /// A "what if crit were 100%" result from the manual Input flow.
    pub critical_rate_assumed: bool,
    pub critical_rate_actual: Option<f64>,
    pub source: Option<String>,
    /// Scouter boss key (`hard_limbo`) → card.
    pub bosses: BTreeMap<String, StoredCard>,
}

/// One boss card of a [`StoredMember`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredCard {
    pub percentage: f64,
    pub rating: String,
    pub party: bool,
    pub effective_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
}

/// A party's additive estimate for one boss key, as the scouter answered
/// `igns=`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredCombined {
    pub combined_percentage: Option<f64>,
    pub estimated_minutes: Option<f64>,
    pub technically_possible: Option<bool>,
    pub complete: bool,
    /// IGN → issue code.
    pub issues: BTreeMap<String, String>,
}

/// One `estimate_members` row.
#[derive(Clone, Debug, PartialEq)]
pub struct MemberEstimate {
    pub character_id: String,
    /// The roster IGN key the result was looked up under.
    pub ign_key: String,
    pub result: StoredMember,
    pub fetched_at: DateTime<Utc>,
}

impl MemberEstimate {
    /// # Errors
    /// [`StoreError::Constraint`] for a row no store may hold.
    pub fn check(&self) -> Result<(), StoreError> {
        if self.character_id.is_empty() || self.ign_key.is_empty() {
            return Err(StoreError::Constraint(
                "estimate needs a character and an IGN key".into(),
            ));
        }
        Ok(())
    }
}

/// One `estimate_parties` row with the characters it includes.
#[derive(Clone, Debug, PartialEq)]
pub struct PartyEstimate {
    /// The sorted, lowercase IGN set.
    pub set_key: String,
    /// The roster characters in the set, ascending and distinct.
    pub character_ids: Vec<String>,
    /// Scouter boss key → total.
    pub combined: BTreeMap<String, StoredCombined>,
    pub fetched_at: DateTime<Utc>,
}

impl PartyEstimate {
    /// # Errors
    /// [`StoreError::Constraint`] for a row no store may hold.
    pub fn check(&self) -> Result<(), StoreError> {
        let ascending = self.character_ids.windows(2).all(|pair| pair[0] < pair[1]);
        if self.set_key.is_empty()
            || self.character_ids.is_empty()
            || self.character_ids.iter().any(String::is_empty)
            || !ascending
        {
            return Err(StoreError::Constraint(
                "party estimate needs a set key and ascending, distinct characters".into(),
            ));
        }
        Ok(())
    }
}

/// What [`super::CharDataStore::estimates`] loads, newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StoredEstimates {
    pub members: Vec<MemberEstimate>,
    pub parties: Vec<PartyEstimate>,
}

/// What [`super::CharDataStore::delete_estimates`] removed, so memory can
/// drop the same entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeletedEstimates {
    /// The deleted result's IGN key; `None` when the character had none.
    pub ign_key: Option<String>,
    /// Every deleted party total naming the character, ascending.
    pub set_keys: Vec<String>,
}
