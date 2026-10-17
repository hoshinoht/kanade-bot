//! A run's party read against the saved estimates (each character's stored
//! result and each boss party's stored total, of any age): the one source
//! of the reminder card's "Who carries" image and of the run reads' Party
//! total, so they never disagree. Boss by boss: a member may bring another
//! character to a later boss, and each boss's figures are that boss's own
//! party. Never starts a lookup.

use chrono::{DateTime, Utc};

use super::{Combined, Member, RosterIgn, Totals, scouter_keys};
use crate::domain::characters::{Character, PartySlot};
use crate::domain::chardata::{BossCarrier, Carrier, PartyShares};

/// What the saved estimates say about a run's party.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SavedParty {
    /// Each member's figures, carriers in party order.
    pub party: PartyShares,
    /// Each boss's stored total of exactly its whole party; `None` while
    /// anyone of it brings no character the scouter can take, or no such
    /// total is stored.
    pub totals: Vec<Option<Combined>>,
    /// When the oldest reading used (members' and totals) was fetched.
    pub oldest: Option<DateTime<Utc>>,
    /// Who each carrier is, in carrier order.
    pub owners: Vec<CarrierOwner>,
    /// Every active character's IGN of every party member, in party order.
    pub igns: Vec<String>,
}

/// The party member behind a carrier and the character their figures were
/// read under (`Carrier::ign`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CarrierOwner {
    pub member_id: String,
    pub character_id: String,
}

impl SavedParty {
    /// Each boss's estimated minutes, from its stored whole-party total.
    pub fn minutes(&self) -> Vec<Option<f64>> {
        self.totals
            .iter()
            .map(|total| total.as_ref().and_then(|total| total.estimated_minutes))
            .collect()
    }

    /// Each boss's "technically possible", from its stored whole-party total.
    pub fn possible(&self) -> Vec<Option<bool>> {
        self.totals
            .iter()
            .map(|total| total.as_ref().and_then(|total| total.technically_possible))
            .collect()
    }
}

/// The IGN a character is looked up under, when the scouter can take it.
fn ign_of(character: Option<&Character>) -> Option<RosterIgn> {
    character.and_then(|character| RosterIgn::lookup(character).ok())
}

/// A brought character the scouter can take.
fn scoutable<B>(found: Option<(&Character, B)>) -> Option<&Character> {
    found
        .map(|(character, _)| character)
        .filter(|character| ign_of(Some(character)).is_some())
}

/// `slots` (of a run with `bosses`) read against the saved estimates
/// (`cached`, `cached_party`), boss by boss: a member bringing a character
/// the scouter can take to some boss is a carrier (with nothing stored, a
/// carrier without figures), numbered once, each boss's figure from the
/// character brought to it; anyone else is missing. Each boss's total is
/// the stored one of that boss's own whole party, asked once per party.
pub fn saved_party(
    bosses: &[String],
    slots: &[PartySlot],
    cached: impl Fn(&RosterIgn) -> Option<(Member, DateTime<Utc>)>,
    cached_party: impl Fn(&[RosterIgn]) -> Option<(Totals, DateTime<Utc>)>,
) -> SavedParty {
    let mut party = PartyShares {
        bosses: bosses.to_vec(),
        ..PartyShares::default()
    };
    let mut oldest: Option<DateTime<Utc>> = None;
    let mut owners = Vec::new();
    let igns = slots
        .iter()
        .flat_map(|slot| {
            slot.candidates
                .iter()
                .map(|character| character.ign.clone())
        })
        .collect();
    let mut seen = |at: DateTime<Utc>| oldest = Some(oldest.map_or(at, |was| was.min(at)));
    // Each boss's own party: the IGNs brought to it, and who brings none.
    let mut parties: Vec<(Vec<RosterIgn>, usize)> = vec![(Vec::new(), 0); bosses.len()];
    for slot in slots {
        let brought: Vec<Option<RosterIgn>> = (0..bosses.len())
            .map(|boss| ign_of(slot.brought_to(boss).map(|(character, _)| character)))
            .collect();
        for (boss, ign) in brought.iter().enumerate() {
            match ign {
                Some(ign) => parties[boss].0.push(ign.clone()),
                None => parties[boss].1 += 1,
            }
        }
        // The first boss's character, else the first one brought to any.
        let first = scoutable(slot.brought())
            .or_else(|| (0..bosses.len()).find_map(|boss| scoutable(slot.brought_to(boss))));
        let (Some(character), Some(ign)) = (first, ign_of(first)) else {
            party.missing += 1;
            continue;
        };
        owners.push(CarrierOwner {
            member_id: slot.member_id.clone(),
            character_id: character.id.clone(),
        });
        let reading = cached(&ign);
        if let Some((_, fetched_at)) = &reading {
            seen(*fetched_at);
        }
        let mut effective = Vec::with_capacity(bosses.len());
        let mut per_boss = Vec::with_capacity(bosses.len());
        for (token, there) in bosses.iter().zip(&brought) {
            let (figure, other) = match there {
                Some(there) if there.key() != ign.key() => {
                    let other = cached(there);
                    if let Some((_, fetched_at)) = &other {
                        seen(*fetched_at);
                    }
                    (
                        other
                            .as_ref()
                            .and_then(|(member, _)| member.effective(token)),
                        Some(BossCarrier {
                            ign: there.as_str().to_owned(),
                            crit_assumed: other
                                .as_ref()
                                .is_some_and(|(member, _)| member.crit_assumed()),
                        }),
                    )
                }
                Some(_) => (
                    reading
                        .as_ref()
                        .and_then(|(member, _)| member.effective(token)),
                    None,
                ),
                None => (None, None),
            };
            effective.push(figure);
            per_boss.push(other);
        }
        if per_boss.iter().all(Option::is_none) {
            per_boss.clear();
        }
        party.carriers.push(Carrier {
            ign: ign.as_str().to_owned(),
            crit_assumed: reading
                .as_ref()
                .is_some_and(|(member, _)| member.crit_assumed()),
            effective,
            per_boss,
        });
    }
    // One stored total per distinct party, each counted for its bosses.
    let mut asked: Vec<(&[RosterIgn], Option<Totals>)> = Vec::new();
    let totals = bosses
        .iter()
        .zip(&parties)
        .map(|(token, (igns, missing))| {
            if *missing > 0 || igns.is_empty() {
                return None;
            }
            let total = match asked.iter().find(|(set, _)| *set == igns.as_slice()) {
                Some((_, total)) => total.clone(),
                None => {
                    let found = cached_party(igns);
                    if let Some((_, fetched_at)) = &found {
                        seen(*fetched_at);
                    }
                    let total = found.map(|(total, _)| total);
                    asked.push((igns, total.clone()));
                    total
                }
            };
            scouter_keys(token).find_map(|key| total.as_ref()?.get(key).cloned())
        })
        .collect();
    SavedParty {
        party,
        totals,
        oldest,
        owners,
        igns,
    }
}
