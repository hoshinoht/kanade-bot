//! Module `character_data`'s 24-hour call budget as the mock's Config shows
//! it: 162 of 800 calls spent, by this morning's batch and the lookups
//! since (mock `character_data::health`).

use super::Seed;
use crate::Fail;
use kanade_domain::chardata::DAILY_CALLS;

const CALLS: i64 = 162;

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    let Some(service) = &seed.state.character_data else {
        return Ok(());
    };
    let store = service.store();
    // One a minute, the newest a minute ago: all inside the window.
    for minute in 1..=CALLS {
        if !store
            .reserve_call(seed.ago(minute * 60), DAILY_CALLS)
            .await?
        {
            return Err("the seeded NEXON calls overflowed the budget".into());
        }
    }
    Ok(())
}
