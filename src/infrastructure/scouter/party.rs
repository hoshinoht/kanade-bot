//! A run's party as the scouter looks it up: the character each member
//! brings (as the admin and portal character reads resolve it), unique by
//! IGN, and the members left out with why. "Look up party" and the
//! background refresh resolve parties the same way.

use super::{Missing, MissingReason, RosterIgn, Subject};
use crate::domain::characters::PartySlot;

/// Unique by IGN (the scouter refuses duplicates).
pub fn push_subject(subjects: &mut Vec<Subject>, subject: Subject) {
    let key = subject.ign.key();
    if !subjects.iter().any(|seen| seen.ign.key() == key) {
        subjects.push(subject);
    }
}

/// `slots`' subjects in party order, and the members who bring no
/// character the scouter can take.
pub fn party_subjects(slots: &[PartySlot]) -> (Vec<Subject>, Vec<Missing>) {
    let mut subjects = Vec::new();
    let mut missing = Vec::new();
    for slot in slots {
        let Some((character, _)) = slot.brought() else {
            missing.push(Missing {
                member_id: slot.member_id.clone(),
                character_id: None,
                reason: if slot.candidates.is_empty() {
                    MissingReason::NoCharacters
                } else {
                    MissingReason::NoCharacterPicked
                },
            });
            continue;
        };
        match RosterIgn::lookup(character) {
            Ok(ign) => push_subject(
                &mut subjects,
                Subject {
                    character_id: character.id.clone(),
                    member_id: character.member_id.clone(),
                    ign,
                },
            ),
            Err(reason) => missing.push(Missing {
                member_id: slot.member_id.clone(),
                character_id: Some(character.id.clone()),
                reason,
            }),
        }
    }
    (subjects, missing)
}
