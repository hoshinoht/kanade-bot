//! Link operations. An admin's link is active at once, with no member
//! confirmation (user decision 2026-10-13), and fetches; a member's own
//! link records their consent to the current notice, then fetches. An
//! unlink deletes the link and its data in one write (the roster keeps the
//! class and level copied from NEXON), makes its posts due and wakes the
//! sweeper. Every change appends a History row with the status words and
//! the actor.

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

/// An admin's link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminLinkOutcome {
    /// Active; how the first fetch went.
    Linked(Box<Link>, FetchState),
    NotConnected,
    /// No such roster character.
    Missing,
    Retired,
    /// Not a MapleSEA character.
    Region,
    /// Already linked, or waiting for its member to confirm a new notice.
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
        // A retired character keeps its data until it expires (refresh is
        // paused, not the data): the strip shows it with its deletion date.
        let snapshot = match &current {
            Some((link, _)) if link.status == LinkStatus::Active => {
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
            // Confirmed by its member under an older notice: pending until
            // they confirm again, whether or not the sweep's re-pend landed.
            // An admin's link is never re-asked.
            let link = if link.needs_reconsent() {
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
    /// an active link on a roster character that still matches it. A retired
    /// character's data is kept, and shown, until it expires.
    pub async fn data(&self, character_id: &str) -> Result<Option<Snapshot>, StoreError> {
        match self.current_link(character_id).await? {
            Some((link, _)) if link.status == LinkStatus::Active => {
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

    /// An admin links `character_id` (expected link version `expected`, 0
    /// for none) for its member: active at once, with no confirmation, then
    /// `id` + `basic` + `stat` run (a refused or spent call leaves it to the
    /// retry or the daily batch, as any refresh). The link records the
    /// notice in force and when, with no consent surface (the member gave
    /// none) and the admin as its requester. A stale link is linked again.
    pub async fn admin_link(
        &self,
        character_id: &str,
        expected: u32,
        actor: &Actor,
        surface: Surface,
    ) -> Result<AdminLinkOutcome, StoreError> {
        if !self.connected() {
            return Ok(AdminLinkOutcome::NotConnected);
        }
        let _slot = self.limiter.character(character_id).await;
        let Some(character) = self.roster.character(character_id.to_owned()).await? else {
            return Ok(AdminLinkOutcome::Missing);
        };
        if !character.is_active() {
            return Ok(AdminLinkOutcome::Retired);
        }
        if character.world != LINK_REGION {
            return Ok(AdminLinkOutcome::Region);
        }
        // A link left over from another identity is removed first.
        let current = self.current_link(character_id).await?.map(|(link, _)| link);
        if current.as_ref().map_or(0, |link| link.version) != expected {
            return Ok(AdminLinkOutcome::Stale(current));
        }
        if let Some(link) = &current
            && link.status != LinkStatus::Stale
        {
            return Ok(AdminLinkOutcome::AlreadyLinked(link.clone()));
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
            version: expected,
        };
        let link = admin_active(link, now);
        match self.store.put_link(link.clone(), expected).await? {
            LinkPut::Written => {}
            LinkPut::Stale(stored) => return Ok(AdminLinkOutcome::Stale(stored)),
        }
        let before = current.map(|link| link.status);
        self.audit(
            character_id,
            before,
            Some(LinkStatus::Active),
            actor,
            surface,
            None,
        )
        .await?;
        log("INFO", "chardata_admin_linked", json!({}));
        let fetched = self.fetch(&link, &character, None).await?;
        Ok(AdminLinkOutcome::Linked(Box::new(link), fetched))
    }

    /// Once at startup: admin requests left pending from before admin
    /// links went active at once become active, by the sweeper (History
    /// `pending` → `linked`), and fetch under the batch's stops. Only a
    /// request on an active MapleSEA character that still matches it; a
    /// member's own link waiting for a new notice is never touched, and
    /// once activated there is nothing left to do, so the pass is
    /// idempotent. Returns how many links became active.
    pub async fn activate_requests(&self) -> u64 {
        if !self.connected() {
            return 0;
        }
        let links = match self.store.links().await {
            Ok(links) => links,
            Err(error) => {
                log(
                    "WARN",
                    "chardata_activation_failed",
                    json!({ "error": super::sweep::error_kind(&error) }),
                );
                return 0;
            }
        };
        let actor = Actor::system(super::SWEEPER);
        let mut activated = Vec::new();
        for link in links.iter().filter(|link| is_admin_request(link)) {
            let id = &link.character_id;
            let _slot = self.limiter.character(id).await;
            let (Ok(Some((link, true))), Ok(Some(character))) = (
                self.current_link(id).await,
                self.roster.character(id.clone()).await,
            ) else {
                continue;
            };
            if !is_admin_request(&link) {
                continue;
            }
            let active = admin_active(link.clone(), self.now());
            if !matches!(
                self.store.put_link(active.clone(), link.version).await,
                Ok(LinkPut::Written)
            ) {
                continue;
            }
            if let Err(error) = self
                .audit(
                    id,
                    Some(LinkStatus::Pending),
                    Some(LinkStatus::Active),
                    &actor,
                    Surface::DeliveryTick,
                    None,
                )
                .await
            {
                log(
                    "WARN",
                    "chardata_activation_unrecorded",
                    json!({ "character": id, "error": super::sweep::error_kind(&error) }),
                );
            }
            activated.push((active, character));
        }
        let mut batch = super::calls::Batch::default();
        let mut fetched = 0;
        for (link, character) in &activated {
            let _slot = self.limiter.character(&link.character_id).await;
            match self.fetch(link, character, Some(&mut batch)).await {
                Ok(FetchState::Stored) => fetched += 1,
                Ok(FetchState::Failed { code, .. }) => {
                    if self.batch_stop(&batch, code).is_some() {
                        break;
                    }
                }
                _ => {}
            }
        }
        log(
            "INFO",
            "chardata_requests_activated",
            json!({ "activated": activated.len(), "fetched": fetched }),
        );
        u64::try_from(activated.len()).unwrap_or(u64::MAX)
    }

    /// The member `member_id` confirms notice `notice_version` for one of
    /// their characters on `surface`: their character (unlinked, stale, or
    /// waiting for them to confirm a new notice) becomes active with the
    /// consent (and where it was given) recorded, then `id` + `basic` +
    /// `stat` run.
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

/// An admin's request still waiting from before admin links went active at
/// once (a member's own link waiting for a new notice is the sweeper's).
fn is_admin_request(link: &Link) -> bool {
    link.status == LinkStatus::Pending && link.requested_by.kind() == "admin"
}

/// `link` made active by an admin at `at`, as its next version: the notice
/// in force and when, with no consent surface (the member gave none).
fn admin_active(link: Link, at: DateTime<Utc>) -> Link {
    Link {
        status: LinkStatus::Active,
        consent_version: Some(NOTICE_VERSION),
        consented_at: Some(at),
        linked_at: Some(at),
        consent_surface: None,
        version: link.version + 1,
        ..link
    }
}
