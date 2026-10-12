//! The only name MapleScouter is ever sent.

use crate::domain::characters::Character;

/// A roster character's IGN, as an admin entered it on the roster, within
/// MapleScouter's rule (1-32 letters or digits; roster IGNs are ASCII).
/// It can only be built from a roster [`Character`], so no Nexon field
/// (ocid, Nexon's `character_name` casing, level, combat power) can reach
/// a scouter request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterIgn(String);

impl RosterIgn {
    /// `None` when the roster IGN is outside MapleScouter's rule.
    pub fn of(character: &Character) -> Option<Self> {
        let ign = character.ign.as_str();
        ((1..=32).contains(&ign.len()) && ign.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then(|| Self(ign.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// MapleScouter compares IGNs case-insensitively (and refuses
    /// duplicates by this key).
    pub fn key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}
