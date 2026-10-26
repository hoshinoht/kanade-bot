//! The admin's Add-character lookup: `id` then `basic` for a typed IGN,
//! answered once and never stored (NEXON terms §8.2/§8.7: a copied value is
//! NEXON data, kept only under a member's consent and for 30 days). It
//! spends the shared 24-hour budget like every call; one admin may look up
//! once per [`LOOKUP_SPACING`].

use chrono::{NaiveDate, TimeDelta};
use serde_json::json;

use super::calls::CallFailed;
use super::refresh::{facts, invalid_identifier, no_such_name};
use super::{CharacterData, log};
use kanade_domain::chardata::{CALL_WINDOW, DAILY_CALLS};
use kanade_domain::history::Actor;
use kanade_domain::scheduler::StoreError;

/// `id` + `basic`: what a lookup needs from the budget.
const LOOKUP_CALLS: u32 = 2;
/// The shortest time between two lookups by one admin (a typing loop or a
/// held Enter key spends the budget otherwise).
pub const LOOKUP_SPACING: TimeDelta = TimeDelta::seconds(3);

/// What NEXON says about a name, shown in the dialog only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub name: Option<String>,
    pub world: Option<String>,
    pub class: Option<String>,
    pub level: Option<u16>,
    pub image_url: Option<String>,
    /// The response date (Singapore), else the lookup day.
    pub as_of: NaiveDate,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupOutcome {
    Found(Found),
    /// NEXON knows no character by that name.
    NotFound,
    NotConnected,
    /// This admin looked up less than [`LOOKUP_SPACING`] ago.
    TooSoon,
    /// Calls are held (key refused, maintenance), the budget is spent or
    /// the call failed: the reason or Open API code.
    Unavailable(String),
}

impl CharacterData {
    /// Look `ign` up for `actor` (a MapleSEA name, already checked). No
    /// link, snapshot or refresh state is written.
    pub async fn look_up(&self, actor: &Actor, ign: &str) -> Result<LookupOutcome, StoreError> {
        if !self.connected() {
            return Ok(LookupOutcome::NotConnected);
        }
        let now = self.now();
        if let Some(held) = self.held(now) {
            return Ok(LookupOutcome::Unavailable(held.label()));
        }
        if self.store.calls_since(now - CALL_WINDOW).await? + LOOKUP_CALLS > DAILY_CALLS {
            return Ok(LookupOutcome::Unavailable(CallFailed::Budget.label()));
        }
        {
            let key = format!("{}:{}", actor.kind(), actor.id());
            let mut holds = self.holds();
            // Spent entries go first, so the map holds recent admins only;
            // one stamped ahead of a clock that stepped back goes too.
            holds
                .lookups
                .retain(|_, at| *at <= now && *at + LOOKUP_SPACING > now);
            if holds.lookups.contains_key(&key) {
                return Ok(LookupOutcome::TooSoon);
            }
            holds.lookups.insert(key, now);
        }
        let outcome = self.fetch_preview(ign).await;
        log(
            "INFO",
            "chardata_lookup",
            json!({ "found": matches!(outcome, LookupOutcome::Found(_)) }),
        );
        Ok(outcome)
    }

    async fn fetch_preview(&self, ign: &str) -> LookupOutcome {
        let ocid = match self.call("id", None, |client| client.id(ign)).await {
            Ok(Some(ocid)) => ocid,
            Ok(None) => return LookupOutcome::NotFound,
            Err(failed) if invalid_identifier(&failed) || no_such_name(&failed) => {
                return LookupOutcome::NotFound;
            }
            Err(failed) => return LookupOutcome::Unavailable(failed.label()),
        };
        let ocid_at = self.now();
        let basic = match self
            .call("character/basic", None, |client| client.basic(&ocid))
            .await
        {
            Ok(basic) => basic,
            Err(failed) if invalid_identifier(&failed) => return LookupOutcome::NotFound,
            Err(failed) => return LookupOutcome::Unavailable(failed.label()),
        };
        // The same bounds and https-only image rule as stored data; the
        // snapshot is built only to read them and is dropped here.
        let shown = facts("", ocid, ocid_at, basic, None, self.now());
        LookupOutcome::Found(Found {
            as_of: shown.as_of(),
            name: shown.character_name,
            world: shown.world_name,
            class: shown.character_class,
            level: shown.level,
            image_url: shown.image_url,
        })
    }
}
