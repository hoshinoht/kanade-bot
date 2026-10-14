//! Link operations. An admin request is `pending` and calls nothing; only
//! the member's confirmation (with the current notice) makes it active and
//! fetches. An unlink deletes the link and its data in one write, makes its
//! posts due and wakes the sweeper. Every change appends a History row with
//! the status words and the actor.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::refresh::FetchState;
use super::{CharacterData, log};
use crate::domain::characters::{Character, ign_key};
use crate::domain::chardata::{
    CALL_WINDOW, ConsentSurface, DAILY_CALLS, LINK_REGION, Link, LinkPut, LinkStatus,
    NOTICE_VERSION, RefreshState, Snapshot, UnlinkReason, link_change, same_identity,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::replays::StoredReplay;

/// `id` + `basic` + `stat`: what a first fetch needs from the budget.
const FIRST_FETCH_CALLS: u32 = 3;

/// Everything the admin strip shows for one character.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkView {
    pub connected: bool,
    pub link: Option<Link>,
    /// Fresh data only (never expired or stamped in the future).
    pub snapshot: Option<Snapshot>,
    pub refresh: Option<RefreshState>,
    pub now: DateTime<Utc>,
}

/// An admin link request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestOutcome {
    /// Now pending (no call made).
    Requested(Link),
    NotConnected,
    /// No such roster character.
    Missing,
    Retired,
    /// Not a MapleSEA character.
    Region,
    /// Already pending or linked.
    AlreadyLinked(Link),
    /// The link is at another version than the one sent (`None`: none).
    Stale(Option<Link>),
}

/// A member's confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfirmOutcome {
    /// Active; how the first fetch went.
    Linked(Box<Link>, FetchState),
    NotConnected,
    /// The member confirmed another notice than the current one.
    NoticeOutdated,
    /// Not one of the member's active MapleSEA roster characters.
    NotEligible,
    AlreadyLinked,
    /// Changed concurrently; ask again.
    Conflict,
    /// The 24-hour budget cannot cover the first fetch: nothing written.
    Busy,
    /// Calls are held (key refused or maintenance): nothing written.
    Unavailable(String),
}

impl CharacterData {
    pub async fn view(&self, character_id: &str) -> Result<LinkView, StoreError> {
        self.read_view(character_id, true).await
    }

    /// [`Self::view`] without the refresh state, for readers that show no
    /// refresh (run readiness).
    pub async fn status(&self, character_id: &str) -> Result<LinkView, StoreError> {
        self.read_view(character_id, false).await
    }

    async fn read_view(&self, character_id: &str, refresh: bool) -> Result<LinkView, StoreError> {
        let now = self.now();
        let current = self.current_link(character_id).await?;
        let snapshot = match &current {
            Some((link, true)) if link.status == LinkStatus::Active => {
                self.store.snapshot(character_id.to_owned(), now).await?
            }
            _ => None,
        };
        let refresh = match &current {
            Some(_) if refresh => self
                .store
                .refresh_states()
                .await?
                .into_iter()
                .find(|state| state.character_id == character_id),
            _ => None,
        };
        Ok(LinkView {
            connected: self.connected(),
            link: current.map(|(link, _)| link),
            snapshot,
            refresh,
            now,
        })
    }

    /// The link, only while it still names its roster character (same
    /// member, IGN and region), with whether that character is active; a
    /// link consented under an older notice reads as pending (its stored
    /// version kept).
    /// Fails closed: a link whose character is gone or changed is never
    /// read (`None`) and is unlinked now; a failed unlink is left to the
    /// sweep's orphan pass. Every read of character data goes through here.
    pub async fn current_link(
        &self,
        character_id: &str,
    ) -> Result<Option<(Link, bool)>, StoreError> {
        let Some(link) = self.store.link(character_id.to_owned()).await? else {
            return Ok(None);
        };
        let character = self.roster.character(character_id.to_owned()).await?;
        if let Some(character) = &character
            && same_identity(&link, character)
        {
            // Confirmed under an older notice: pending until the member
            // confirms again, whether or not the sweep's re-pend landed.
            let link = if link.status != LinkStatus::Pending
                && link.consent_version != Some(NOTICE_VERSION)
            {
                Link {
                    status: LinkStatus::Pending,
                    consent_version: None,
                    consented_at: None,
                    linked_at: None,
                    consent_surface: None,
                    ..link
                }
            } else {
                link
            };
            return Ok(Some((link, character.is_active())));
        }
        let reason = if character.is_none() {
            UnlinkReason::CharacterGone
        } else {
            UnlinkReason::IdentityChanged
        };
        let actor = Actor::system(super::SWEEPER);
        if self
            .unlink(character_id, &actor, Surface::DeliveryTick, reason, None)
            .await
            .is_err()
        {
            log(
                "WARN",
                "chardata_unlink_deferred",
                json!({ "character": character_id, "reason": reason.as_str() }),
            );
        }
        Ok(None)
    }

    /// The fresh data a reader may show (strip, readiness, posts): only for
    /// an active link on an active roster character that still matches it.
    pub async fn data(&self, character_id: &str) -> Result<Option<Snapshot>, StoreError> {
        match self.current_link(character_id).await? {
            Some((link, true)) if link.status == LinkStatus::Active => {
                self.store
                    .snapshot(character_id.to_owned(), self.now())
                    .await
            }
            _ => Ok(None),
        }
    }

    /// Append the History row of a link moving `before` → `after`.
    async fn audit(
        &self,
        character_id: &str,
        before: Option<LinkStatus>,
        after: Option<LinkStatus>,
        actor: &Actor,
        surface: Surface,
        replay: Option<StoredReplay>,
    ) -> Result<(), StoreError> {
        match link_change(character_id, before, after, actor, surface, self.now()) {
            Some(change) => self.roster.record(change, replay).await,
            None => Ok(()),
        }
    }

    /// An admin asks to link `character_id` (expected link version
    /// `expected`, 0 for none). Pending only: no Nexon call until the
    /// member confirms. `replay` builds a keyed request's stored answer
    /// from the new link, written with the History row.
    pub async fn request_link(
        &self,
        character_id: &str,
        expected: u32,
        actor: &Actor,
        surface: Surface,
        replay: impl FnOnce(&Link) -> Option<StoredReplay> + Send,
    ) -> Result<RequestOutcome, StoreError> {
        if !self.connected() {
            return Ok(RequestOutcome::NotConnected);
        }
        let Some(character) = self.roster.character(character_id.to_owned()).await? else {
            return Ok(RequestOutcome::Missing);
        };
        if !character.is_active() {
            return Ok(RequestOutcome::Retired);
        }
        if character.world != LINK_REGION {
            return Ok(RequestOutcome::Region);
        }
        // A link left over from another identity is removed first.
        let current = self.current_link(character_id).await?.map(|(link, _)| link);
        if current.as_ref().map_or(0, |link| link.version) != expected {
            return Ok(RequestOutcome::Stale(current));
        }
        if let Some(link) = &current
            && link.status != LinkStatus::Stale
        {
            return Ok(RequestOutcome::AlreadyLinked(link.clone()));
        }
        let now = self.now();
        let link = Link {
            character_id: character_id.to_owned(),
            member_id: character.member_id.clone(),
            ign_key: ign_key(&character.ign),
            region: LINK_REGION.into(),
            status: LinkStatus::Pending,
            requested_by: actor.clone(),
            requested_at: now,
            consent_version: None,
            consented_at: None,
            linked_at: None,
            consent_surface: None,
            version: expected + 1,
        };
        match self.store.put_link(link.clone(), expected).await? {
            LinkPut::Written => {}
            LinkPut::Stale(stored) => return Ok(RequestOutcome::Stale(stored)),
        }
        let before = current.map(|link| link.status);
        self.audit(
            character_id,
            before,
            Some(LinkStatus::Pending),
            actor,
            surface,
            replay(&link),
        )
        .await?;
        log("INFO", "chardata_link_requested", json!({}));
        Ok(RequestOutcome::Requested(link))
    }

    /// The member `member_id` confirms notice `notice_version` for one of
    /// their characters on `surface`: a pending request (or, with none,
    /// their own active character directly) becomes active with the consent
    /// (and where it was given) recorded, then `id` + `basic` + `stat` run.
    /// Discord's `/character link` and the portal's Link call this.
    pub async fn confirm_link(
        &self,
        member_id: &str,
        character_id: &str,
        notice_version: u32,
        surface: ConsentSurface,
    ) -> Result<ConfirmOutcome, StoreError> {
        if !self.connected() {
            return Ok(ConfirmOutcome::NotConnected);
        }
        if notice_version != NOTICE_VERSION {
            return Ok(ConfirmOutcome::NoticeOutdated);
        }
        let _slot = self.limiter.character(character_id).await;
        let Some(character) = self.roster.character(character_id.to_owned()).await? else {
            return Ok(ConfirmOutcome::NotEligible);
        };
        if character.member_id != member_id
            || !character.is_active()
            || character.world != LINK_REGION
        {
            return Ok(ConfirmOutcome::NotEligible);
        }
        let current = self.current_link(character_id).await?.map(|(link, _)| link);
        if current
            .as_ref()
            .is_some_and(|link| link.status == LinkStatus::Active)
        {
            return Ok(ConfirmOutcome::AlreadyLinked);
        }
        let now = self.now();
        // Checked before the consent is written, so a link never becomes
        // active only to wait a day for its first data.
        if let Some(held) = self.held(now) {
            return Ok(ConfirmOutcome::Unavailable(held.label()));
        }
        if self.store.calls_since(now - CALL_WINDOW).await? + FIRST_FETCH_CALLS > DAILY_CALLS {
            return Ok(ConfirmOutcome::Busy);
        }
        let actor = Actor::member(member_id);
        let expected = current.as_ref().map_or(0, |link| link.version);
        let link = Link {
            character_id: character_id.to_owned(),
            member_id: member_id.to_owned(),
            ign_key: ign_key(&character.ign),
            region: LINK_REGION.into(),
            status: LinkStatus::Active,
            requested_by: current
                .as_ref()
                .map_or_else(|| actor.clone(), |link| link.requested_by.clone()),
            requested_at: current.as_ref().map_or(now, |link| link.requested_at),
            consent_version: Some(NOTICE_VERSION),
            consented_at: Some(now),
            linked_at: Some(now),
            consent_surface: Some(surface),
            version: expected + 1,
        };
        if self.store.put_link(link.clone(), expected).await? != LinkPut::Written {
            return Ok(ConfirmOutcome::Conflict);
        }
        let before = current.map(|link| link.status);
        self.audit(
            character_id,
            before,
            Some(LinkStatus::Active),
            &actor,
            surface.history(),
            None,
        )
        .await?;
        log("INFO", "chardata_link_confirmed", json!({}));
        let fetched = self.fetch(&link, &character, None).await?;
        Ok(ConfirmOutcome::Linked(Box::new(link), fetched))
    }

    /// Delete the link and its data now (one write), make its posts due,
    /// record History and wake the sweeper. `None` when it was not linked.
    pub async fn unlink(
        &self,
        character_id: &str,
        actor: &Actor,
        surface: Surface,
        reason: UnlinkReason,
        replay: Option<StoredReplay>,
    ) -> Result<Option<Link>, StoreError> {
        let removed = self
            .store
            .unlink(character_id.to_owned(), self.now())
            .await?;
        if let Some(link) = &removed {
            self.wake.notify_one();
            log(
                "INFO",
                "chardata_unlinked",
                json!({ "reason": reason.as_str() }),
            );
            self.audit(
                character_id,
                Some(link.status),
                None,
                actor,
                surface,
                replay,
            )
            .await?;
        }
        Ok(removed)
    }

    /// After a committed roster write: a deleted character, or one whose
    /// IGN, region or member changed, loses its link at once. Failures are
    /// logged; the sweeper's orphan pass catches what is left.
    pub async fn roster_changed(
        &self,
        character_id: &str,
        after: Option<&Character>,
        actor: &Actor,
        surface: Surface,
    ) {
        let link = match self.store.link(character_id.to_owned()).await {
            Ok(Some(link)) => link,
            Ok(None) => return,
            Err(_) => {
                log("WARN", "chardata_roster_unlink_failed", json!({}));
                return;
            }
        };
        let reason = match after {
            None => UnlinkReason::CharacterGone,
            Some(after) if !same_identity(&link, after) => UnlinkReason::IdentityChanged,
            Some(_) => return,
        };
        if self
            .unlink(character_id, actor, surface, reason, None)
            .await
            .is_err()
        {
            log("WARN", "chardata_roster_unlink_failed", json!({}));
        }
    }
}
