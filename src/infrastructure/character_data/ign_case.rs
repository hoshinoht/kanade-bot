//! Roster IGNs of linked characters take NEXON's letter case (user decision
//! 2026-10-13): when a stored snapshot's name equals the roster IGN
//! case-insensitively but not exactly, the roster character is renamed to
//! NEXON's spelling. Only the case of the member-entered IGN changes and
//! nothing else is copied; a different name already makes the link stale.
//! The link keys on the case-insensitive IGN, so it stays as it is. No
//! Nexon call of its own.

use serde_json::json;

use super::sweep::error_kind;
use super::{CharacterData, log};
use crate::domain::characters::{
    Character, CharacterChange, CharacterRecord, CharacterWrite, ign_key,
};
use crate::domain::chardata::{Link, LinkStatus};
use crate::domain::history::{Actor, Surface};

/// The History actor of a case correction: the spelling comes from NEXON.
pub const NEXON_ACTOR: &str = "nexon";

impl CharacterData {
    /// Give `character` (the roster row read before the fetch that stored
    /// `name`) NEXON's spelling when only the letter case differs. A roster
    /// write since that read (another version) skips quietly; a failure is
    /// logged and the next fetch or startup tries again. `true` when the
    /// IGN was corrected.
    pub(super) async fn match_ign_case(
        &self,
        link: &Link,
        character: &Character,
        name: &str,
    ) -> bool {
        let key = ign_key(name);
        if name == character.ign || key != link.ign_key || key != character.ign_key() {
            return false;
        }
        let now = self.now();
        let actor = Actor::system(NEXON_ACTOR);
        let after = Character {
            ign: name.to_owned(),
            ..character.clone()
        }
        .next_version(now, &actor);
        let change = CharacterChange::Update {
            version: character.version,
            after,
        };
        let record = CharacterRecord {
            at: now,
            actor,
            surface: Surface::DeliveryTick,
        };
        match self.roster.write_character(change, record).await {
            Ok(CharacterWrite::Updated(_)) => {
                log("INFO", "chardata_ign_cased", json!({}));
                true
            }
            // Changed meanwhile (or gone): the next fetch looks again.
            Ok(_) => false,
            Err(error) => {
                log(
                    "WARN",
                    "chardata_ign_case_failed",
                    json!({ "error": error_kind(&error) }),
                );
                false
            }
        }
    }

    /// Once at startup: every active link whose fresh snapshot already
    /// names its character in another case. Pending, stale and expired
    /// links (and links read as pending under an older notice) are left
    /// alone. Returns how many IGNs were corrected.
    pub async fn match_ign_cases(&self) -> u64 {
        let links = match self.store.links().await {
            Ok(links) => links,
            Err(error) => {
                log(
                    "WARN",
                    "chardata_ign_cases_failed",
                    json!({ "error": error_kind(&error) }),
                );
                return 0;
            }
        };
        let mut corrected = 0;
        for link in links
            .iter()
            .filter(|link| link.status == LinkStatus::Active)
        {
            let id = &link.character_id;
            // Never beside a fetch of the same character.
            let _slot = self.limiter.character(id).await;
            let Ok(Some((link, character))) = self.refreshable(id).await else {
                continue;
            };
            let Ok(Some(snapshot)) = self.store.snapshot(id.clone(), self.now()).await else {
                continue;
            };
            if let Some(name) = &snapshot.character_name
                && self.match_ign_case(&link, &character, name).await
            {
                corrected += 1;
            }
        }
        log(
            "INFO",
            "chardata_ign_cases",
            json!({ "corrected": corrected }),
        );
        corrected
    }
}
