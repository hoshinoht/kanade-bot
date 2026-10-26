//! The only name the scouter is ever sent.

use super::MissingReason;
use kanade_domain::characters::{Character, DEFAULT_WORLD};

/// A roster character's IGN, as an admin entered it on the roster, within
/// the scouter's rule (1-32 letters or digits; roster IGNs are ASCII).
/// It can only be built from a roster [`Character`], so no Nexon field
/// (ocid, Nexon's `character_name` casing, level, combat power) can reach
/// a scouter request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterIgn(String);

impl RosterIgn {
    /// `None` when the roster IGN is outside the scouter's rule.
    pub fn of(character: &Character) -> Option<Self> {
        let ign = character.ign.as_str();
        ((1..=32).contains(&ign.len()) && ign.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then(|| Self(ign.to_owned()))
    }

    /// What `character` is to the scouter: an IGN to look up, or why not.
    /// Lookups and every cached read use the same rule.
    pub fn lookup(character: &Character) -> Result<Self, MissingReason> {
        if character.world != DEFAULT_WORLD {
            return Err(MissingReason::OtherRegion);
        }
        Self::of(character).ok_or(MissingReason::UnsupportedIgn)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The scouter compares IGNs case-insensitively (and refuses
    /// duplicates by this key).
    pub fn key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}
