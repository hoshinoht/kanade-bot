//! The sweeper (hourly and on every unlink): expired and future-stamped
//! snapshots, the orphan pass (with pending requests past 14 days), due
//! posts stripped through the Discord side, then call records older than
//! the window.

use chrono::{DateTime, Utc};
use serde_json::json;

use super::{CharacterData, StripReport, log};
use crate::domain::chardata::{
    CALL_WINDOW, LinkStatus, Orphan, UnlinkReason, orphan, pending_expired,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;

/// What one sweep did; `None` for a step whose store call failed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub snapshots: Option<u64>,
    pub unlinked: u64,
    pub reconsent: u64,
    /// Pending requests removed after 14 days.
    pub expired: u64,
    pub posts: StripReport,
    pub calls: Option<u64>,
}

/// The sweeper's History actor.
pub const SWEEPER: &str = "chardata";

impl CharacterData {
    pub async fn sweep(&self) -> SweepReport {
        let now = self.now();
        let mut report = SweepReport {
            snapshots: self.store.sweep_snapshots(now).await.ok(),
            ..SweepReport::default()
        };
        self.orphans(now, &mut report).await;
        // After the orphan pass, so the posts of what it unlinked go now.
        report.posts = self.strip_posts().await;
        report.calls = self.store.prune_calls(now - CALL_WINDOW).await.ok();
        self.holds().last_sweep = Some(now);
        log(
            "INFO",
            "chardata_sweep",
            json!({
                "snapshots": report.snapshots,
                "unlinked": report.unlinked,
                "reconsent": report.reconsent,
                "expired": report.expired,
                "stripped": report.posts.stripped,
                "strip_retrying": report.posts.retrying,
                "stuck": report.posts.stuck,
                "unknown": report.posts.unknown,
                "calls": report.calls,
            }),
        );
        report
    }

    /// Links whose roster character is gone, changed, retired with expired
    /// data, or whose member left are deleted; links confirmed under an
    /// older notice go back to pending. Catches crashes, missed hooks and
    /// main-store restores.
    async fn orphans(&self, now: DateTime<Utc>, report: &mut SweepReport) {
        let Ok(links) = self.store.links().await else {
            return;
        };
        let actor = Actor::system(SWEEPER);
        for link in links {
            let id = link.character_id.clone();
            // An admin's request the member never confirmed: History says
            // `pending` → `unlinked`, nothing else.
            if pending_expired(&link, now) {
                if let Ok(Some(_)) = self
                    .unlink(
                        &id,
                        &actor,
                        Surface::DeliveryTick,
                        UnlinkReason::PendingExpired,
                        None,
                    )
                    .await
                {
                    report.expired += 1;
                }
                continue;
            }
            let (Ok(character), Ok(member), Ok(fresh)) = (
                self.roster.character(id.clone()).await,
                self.roster.member(link.member_id.clone()).await,
                self.store.snapshot(id.clone(), now).await,
            ) else {
                continue;
            };
            match orphan(&link, character.as_ref(), member.as_ref(), fresh.as_ref()) {
                Orphan::Keep => {}
                Orphan::Unlink(reason) => {
                    if let Ok(Some(_)) = self
                        .unlink(&id, &actor, Surface::DeliveryTick, reason, None)
                        .await
                    {
                        report.unlinked += 1;
                    }
                }
                Orphan::Reconsent => {
                    // History first, then the file. Reads already treat the
                    // link as pending (`current_link`), so a failed step only
                    // delays the file catching up; the next sweep retries,
                    // skipping the History row it already wrote: the
                    // sweeper's last `link.<id>` row says `pending` and is
                    // newer than the link's consent.
                    let warn = |event: &'static str, error: &StoreError| {
                        log(
                            "WARN",
                            event,
                            json!({ "character": id, "error": error_kind(error) }),
                        );
                    };
                    let recorded =
                        match self.roster.last_link_audit(id.clone(), actor.clone()).await {
                            Ok(last) => last.is_some_and(|change| {
                                change
                                    .values
                                    .get(&format!("link.{id}"))
                                    .is_some_and(|diff| diff.to == "pending")
                                    && link.consented_at.is_none_or(|at| change.at >= at)
                            }),
                            Err(error) => {
                                warn("chardata_reconsent_unrecorded", &error);
                                continue;
                            }
                        };
                    let change = crate::domain::chardata::link_change(
                        &id,
                        Some(link.status),
                        Some(LinkStatus::Pending),
                        &actor,
                        Surface::DeliveryTick,
                        now,
                    );
                    if !recorded
                        && let Some(change) = change
                        && let Err(error) = self.roster.record(change, None).await
                    {
                        warn("chardata_reconsent_unrecorded", &error);
                        continue;
                    }
                    let mut pending = link.clone().with_status(LinkStatus::Pending);
                    pending.consent_version = None;
                    pending.consented_at = None;
                    pending.linked_at = None;
                    // A new request by the sweeper: its 14 days start now,
                    // not at the original request.
                    pending.requested_by = actor.clone();
                    pending.requested_at = now;
                    match self.store.put_link(pending, link.version).await {
                        Ok(crate::domain::chardata::LinkPut::Written) => report.reconsent += 1,
                        // Changed concurrently (a fresh confirm): nothing to do.
                        Ok(crate::domain::chardata::LinkPut::Stale(_)) => {}
                        Err(error) => warn("chardata_reconsent_unwritten", &error),
                    }
                }
            }
        }
    }
}

/// A store error's class for logs (its text may carry paths).
fn error_kind(error: &StoreError) -> &'static str {
    match error {
        StoreError::Constraint(_) => "constraint",
        StoreError::Conflict { .. } => "conflict",
        StoreError::Backend(_) => "backend",
        _ => "other",
    }
}
