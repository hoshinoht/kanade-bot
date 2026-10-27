//! A run's party as the scouter looks it up: the character each member
//! brings to each boss (as the admin and portal character reads resolve
//! it), unique by IGN, the members left out with why, and the distinct
//! party of each boss (most runs bring one party to every boss). "Look up
//! party" and the background refresh resolve parties the same way.

use super::{Missing, MissingReason, RosterIgn, Subject};
use kanade_domain::characters::{Character, PartySlot};

/// The characters one or more of a run's bosses bring: the job's subjects
/// that party is made of, in party order, and how many of its members
/// bring none the scouter can take. Its total is asked once and counts
/// only for its bosses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BossParty {
    /// The run's boss tokens it plays, in run order; empty for a lookup of
    /// characters (every boss).
    pub bosses: Vec<String>,
    /// Indexes into the job's subjects.
    pub subjects: Vec<usize>,
    /// Members of it with no character the scouter can take.
    pub missing: usize,
}

impl BossParty {
    /// One party of every subject, for every boss.
    pub fn whole(subjects: usize) -> Self {
        Self {
            bosses: Vec::new(),
            subjects: (0..subjects).collect(),
            missing: 0,
        }
    }
}

/// Unique by IGN (the scouter refuses duplicates); the subject's index.
pub fn push_subject(subjects: &mut Vec<Subject>, subject: Subject) -> usize {
    let key = subject.ign.key();
    match subjects.iter().position(|seen| seen.ign.key() == key) {
        Some(at) => at,
        None => {
            subjects.push(subject);
            subjects.len() - 1
        }
    }
}

/// What a member brings to one boss as the scouter takes it.
enum Brought {
    Subject(usize),
    Missing(Missing),
}

fn bring(subjects: &mut Vec<Subject>, slot: &PartySlot, character: Option<&Character>) -> Brought {
    let Some(character) = character else {
        return Brought::Missing(Missing {
            member_id: slot.member_id.clone(),
            character_id: None,
            reason: if slot.candidates.is_empty() {
                MissingReason::NoCharacters
            } else {
                MissingReason::NoCharacterPicked
            },
        });
    };
    match RosterIgn::lookup(character) {
        Ok(ign) => Brought::Subject(push_subject(
            subjects,
            Subject {
                character_id: character.id.clone(),
                member_id: character.member_id.clone(),
                ign,
            },
        )),
        Err(reason) => Brought::Missing(Missing {
            member_id: slot.member_id.clone(),
            character_id: Some(character.id.clone()),
            reason,
        }),
    }
}

/// `slots`' subjects for `bosses` (the run's tokens): every character
/// brought to any boss, member by member in party order (each member's
/// first-boss character first), unique by IGN; the members who bring no
/// character the scouter can take to some boss (once per member and
/// character); and each distinct boss party, in the order of its first
/// boss. With no bosses, each member's first-boss character is one party.
pub fn party_subjects(
    slots: &[PartySlot],
    bosses: &[String],
) -> (Vec<Subject>, Vec<Missing>, Vec<BossParty>) {
    let mut subjects = Vec::new();
    let mut missing: Vec<Missing> = Vec::new();
    let count = bosses.len().max(1);
    // Per member, what they bring to each boss.
    let mut brought: Vec<Vec<Brought>> = Vec::with_capacity(slots.len());
    for slot in slots {
        let mut row = Vec::with_capacity(count);
        for boss in 0..count {
            let character = slot.brought_to(boss).map(|(character, _)| character);
            let one = bring(&mut subjects, slot, character);
            if let Brought::Missing(left) = &one
                && !missing.iter().any(|seen| {
                    seen.member_id == left.member_id && seen.character_id == left.character_id
                })
            {
                missing.push(left.clone());
            }
            row.push(one);
        }
        brought.push(row);
    }
    let mut parties: Vec<BossParty> = Vec::new();
    for boss in 0..count {
        let mut party = Vec::new();
        let mut short = 0;
        for row in &brought {
            match &row[boss] {
                Brought::Subject(at) => party.push(*at),
                Brought::Missing(_) => short += 1,
            }
        }
        let token = bosses.get(boss).cloned();
        match parties
            .iter_mut()
            .find(|seen| seen.subjects == party && seen.missing == short)
        {
            Some(seen) => seen.bosses.extend(token),
            None => parties.push(BossParty {
                bosses: token.into_iter().collect(),
                subjects: party,
                missing: short,
            }),
        }
    }
    (subjects, missing, parties)
}
