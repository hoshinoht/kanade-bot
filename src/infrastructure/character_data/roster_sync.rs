//! Linked roster characters take NEXON's facts (user decisions 2026-10-13):
//! after each stored fetch the roster row gets NEXON's class and level
//! ("copy and keep": they are roster values from then on and stay after an
//! unlink or expiry) and, when the names differ only in letter case,
//! NEXON's spelling of the IGN. A different name already makes the link
//! stale, and the link keys on the case-insensitive IGN, so it stays as it
//! is. A value NEXON did not answer, or one the roster rules refuse (a class
//! over 32 characters, a level outside 1-300), leaves the roster's own. One
//! version-checked roster write with its History row by [`NEXON_ACTOR`]; no
//! Nexon call of its own.

use serde_json::json;

use super::sweep::error_kind;
use super::{CharacterData, log};
use crate::domain::characters::{
    Character, CharacterChange, CharacterRecord, CharacterWrite, LEVELS, ign_key, parse_class,
};
use crate::domain::chardata::{Link, LinkStatus, Snapshot};
use crate::domain::history::{Actor, Surface};

/// The History actor of a roster write whose values come from NEXON.
pub const NEXON_ACTOR: &str = "nexon";

/// `character` with what it takes from `snapshot` (fetched for `link`), as
/// the same version; `None` when nothing would change.
pub(super) fn synced(link: &Link, character: &Character, snapshot: &Snapshot) -> Option<Character> {
    let mut after = character.clone();
    if let Some(name) = snapshot.character_name.as_deref() {
        let key = ign_key(name);
        if key == link.ign_key && key == character.ign_key() {
            name.clone_into(&mut after.ign);
        }
    }
    if let Some(Ok(Some(class))) = snapshot.character_class.as_deref().map(parse_class) {
        after.class = Some(class);
    }
    if let Some(level) = snapshot.level.filter(|level| LEVELS.contains(level)) {
        after.level = Some(level);
    }
    (!after.same_content(character)).then_some(after)
}

impl CharacterData {
    /// Write `after` (from [`synced`]) over `character`, the roster row read
    /// before the fetch. A roster write since that read (another version)
    /// skips quietly; a failure is logged; the next fetch or startup tries
    /// again either way. `true` when the roster row was written.
    pub(super) async fn sync_roster(&self, character: &Character, after: Character) -> bool {
        let now = self.now();
        let actor = Actor::system(NEXON_ACTOR);
        let change = CharacterChange::Update {
            version: character.version,
            after: after.next_version(now, &actor),
        };
        let record = CharacterRecord {
            at: now,
            actor,
            surface: Surface::DeliveryTick,
        };
        match self.roster.write_character(change, record).await {
            Ok(CharacterWrite::Updated(_)) => {
                log("INFO", "chardata_roster_synced", json!({}));
                true
            }
            // Changed meanwhile (or gone): the next fetch looks again.
            Ok(_) => false,
            Err(error) => {
                log(
                    "WARN",
                    "chardata_roster_sync_failed",
                    json!({ "error": error_kind(&error) }),
                );
                false
            }
        }
    }

    /// Once at startup: every active link on an active roster character
    /// whose fresh snapshot already holds what the roster row lacks (links
    /// fetched before the copy, or whose write was skipped). Pending, stale
    /// and expired links (and links read as pending under an older notice)
    /// are left alone. Returns how many roster rows were written.
    pub async fn sync_rosters(&self) -> u64 {
        let links = match self.store.links().await {
            Ok(links) => links,
            Err(error) => {
                log(
                    "WARN",
                    "chardata_roster_syncs_failed",
                    json!({ "error": error_kind(&error) }),
                );
                return 0;
            }
        };
        let mut written = 0;
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
            if let Some(after) = synced(&link, &character, &snapshot)
                && self.sync_roster(&character, after).await
            {
                written += 1;
            }
        }
        log(
            "INFO",
            "chardata_roster_syncs",
            json!({ "written": written }),
        );
        written
    }
}
