//! What a run's party slots are read from besides the schedule: each party
//! member's characters and the stored picks on the run's boss week and its
//! timings. The admin and portal character reads and the reminder cards'
//! estimate chart load it the same way.

use std::collections::BTreeMap;

use super::{Character, CharacterStore, PartySlot, Pick, PickScope, StoredPicks, party_slots};
use crate::domain::catalog::BossTable;
use crate::domain::schedule::{FixedRun, Run};
use crate::domain::scheduler::StoreError;

/// The roster reads a run's party needs.
pub trait PartyReads {
    fn member_characters(
        &self,
        member_id: &str,
    ) -> impl Future<Output = Result<Vec<Character>, StoreError>> + Send;

    fn picks(
        &self,
        scope: PickScope,
        targets: &[String],
    ) -> impl Future<Output = Result<Vec<Pick>, StoreError>> + Send;
}

impl<T: CharacterStore + Sync> PartyReads for T {
    fn member_characters(
        &self,
        member_id: &str,
    ) -> impl Future<Output = Result<Vec<Character>, StoreError>> + Send {
        CharacterStore::member_characters(self, member_id)
    }

    fn picks(
        &self,
        scope: PickScope,
        targets: &[String],
    ) -> impl Future<Output = Result<Vec<Pick>, StoreError>> + Send {
        CharacterStore::picks(self, scope, targets)
    }
}

/// A run's party members' characters (retired included) and the stored
/// run picks on its boss week's runs and timing picks on their timings.
#[derive(Clone, Debug, Default)]
pub struct PartyRoster {
    pub characters: BTreeMap<String, Vec<Character>>,
    pub picks: Vec<Pick>,
}

impl PartyRoster {
    /// `run`'s party as read now; `week` is every run of its boss week
    /// (`run` among them).
    pub async fn load<R: PartyReads + Sync + ?Sized>(
        roster: &R,
        run: &Run,
        week: &[Run],
    ) -> Result<Self, StoreError> {
        let mut characters = BTreeMap::new();
        for member in &run.participants {
            characters.insert(member.clone(), roster.member_characters(member).await?);
        }
        let ids: Vec<String> = week.iter().map(|run| run.id.clone()).collect();
        let mut picks = roster.picks(PickScope::Run, &ids).await?;
        let mut fixed: Vec<String> = week
            .iter()
            .filter_map(|run| run.fixed_run_id.clone())
            .collect();
        fixed.sort();
        fixed.dedup();
        if !fixed.is_empty() {
            picks.extend(roster.picks(PickScope::Timing, &fixed).await?);
        }
        Ok(Self { characters, picks })
    }

    /// `run`'s slots in party order (see [`party_slots`]).
    pub fn slots(
        &self,
        run: &Run,
        week: &[Run],
        timings: &[FixedRun],
        catalog: &BossTable,
    ) -> Vec<PartySlot> {
        party_slots(
            run,
            week,
            &self.characters,
            &StoredPicks::new(&self.picks, timings),
            catalog,
        )
    }
}
