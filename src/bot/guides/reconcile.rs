//! Unconfirmed creates (plan 5.1, 5.3, 5.5): a create that may have posted
//! is never sent again. It is the journal's bound post when one exists
//! (`bound_source`), else the bot's own message showing the slot's output,
//! newer than every earlier message of the guide, in the channel's recent
//! history (read twice). Exactly one match is the slot; none or several
//! leave it unknown, and the guide waits for an admin ("Search again", a
//! run of kind `search`, or "Mark as not posted").

use std::collections::HashSet;

use tokio::sync::watch;

use super::execute::after;
use super::inspect::deadline;
use super::{Publisher, SEARCH_DEPTH, SEARCHES, payload};
use crate::bot::ids::{id_text, parse_id};
use crate::bot::transport::{DiscordTransport, HistoryPage, Outcome};
use kanade_domain::guides::{
    Guide, GuideStore, Inspection, OpOutcome, PlanOp, Publish, PublishOp, PublishState,
    PublishedSlot, RenderedSlot, SlotEffect, SlotState, StopReason, UploadRef,
};
use kanade_domain::notify::DeliveryJournal;
use kanade_domain::scheduler::StoreError;

/// What a search found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Found {
    One(String),
    /// None, or several (the candidates).
    Not(Vec<String>),
}

/// One unconfirmed create to resolve.
struct Pending<'a> {
    publish_id: &'a str,
    ordinal: u32,
    position: u32,
    channel_id: &'a str,
    hash: &'a str,
    source: &'a str,
    /// Kept on an unknown slot when the draft no longer renders this output.
    uploads: Vec<UploadRef>,
}

impl<S, T> Publisher<S, T>
where
    S: GuideStore + DeliveryJournal + Send + Sync + 'static,
    T: DiscordTransport + 'static,
{
    /// The bot's own messages in `channel_id`'s latest history that show
    /// `slot` and are newer than `after`.
    pub(super) async fn search(
        &self,
        channel_id: &str,
        slot: Option<&RenderedSlot>,
        after: Option<&str>,
    ) -> Found {
        let (Some(channel), Some(me), Some(slot)) = (parse_id(channel_id), (self.self_id)(), slot)
        else {
            return Found::Not(Vec::new());
        };
        let after = after.and_then(|id| id.parse::<u64>().ok()).unwrap_or(0);
        for search in 0..SEARCHES {
            if search > 0 {
                tokio::time::sleep(self.pauses.search).await;
            }
            let Outcome::Delivered(messages) = deadline(self.transport.channel_messages(
                channel,
                HistoryPage::Latest,
                SEARCH_DEPTH,
            ))
            .await
            else {
                continue;
            };
            let mut seen = HashSet::new();
            let candidates: Vec<String> = messages
                .iter()
                .filter(|message| message.id.get() > after && seen.insert(message.id))
                .filter(|message| payload::shows(message, me, slot))
                .map(|message| id_text(message.id))
                .collect();
            match candidates.len() {
                0 => {}
                1 => return Found::One(candidates.into_iter().next().unwrap_or_default()),
                _ => return Found::Not(candidates),
            }
        }
        Found::Not(Vec::new())
    }

    /// Settle one unconfirmed create: the bound post, else the search.
    /// `Ok(true)` when the slot is posted now.
    async fn resolve(
        &self,
        pending: Pending<'_>,
        desired: &[RenderedSlot],
        known: &[PublishedSlot],
    ) -> Result<bool, StoreError> {
        let slot = desired
            .get(pending.position as usize)
            .filter(|slot| slot.hash == pending.hash);
        let bound = match self.store.bound_source(pending.source, 0).await {
            Ok(receipt) => receipt
                .filter(|receipt| receipt.channel_id == pending.channel_id)
                .map(|receipt| receipt.message_id),
            Err(error) => {
                kanade_log::log!(warn, "guide_bound_source_failed", error = error.to_string());
                None
            }
        };
        let found = match bound {
            Some(message_id) => Found::One(message_id),
            None => {
                let after = after(known, pending.channel_id, None);
                self.search(pending.channel_id, slot, after.as_deref())
                    .await
            }
        };
        let (outcome, state, message_id, candidates) = match found {
            Found::One(id) => (
                OpOutcome::Recovered,
                SlotState::Posted,
                Some(id),
                Vec::new(),
            ),
            Found::Not(candidates) => (OpOutcome::Unknown, SlotState::Unknown, None, candidates),
        };
        let row = PublishedSlot {
            position: pending.position,
            channel_id: pending.channel_id.to_owned(),
            message_id,
            hash: pending.hash.to_owned(),
            uploads: slot.map_or(pending.uploads, RenderedSlot::manifest),
            state,
            source: (state == SlotState::Unknown).then(|| pending.source.to_owned()),
            publish_id: pending.publish_id.to_owned(),
            updated_at: self.now(),
        };
        self.store
            .apply_op_result(
                pending.publish_id.to_owned(),
                pending.ordinal,
                outcome,
                SlotEffect::Put(row),
                candidates,
                self.now(),
            )
            .await?;
        Ok(state == SlotState::Posted)
    }

    /// A run of kind `search` ("Search again", 5.3): each unknown slot of
    /// the guide in the run's channel is looked for again; one still unknown
    /// stops the run `unknown_slot`. Before any Discord call the saved map
    /// is re-read (nothing left unknown: `plan_changed`, nothing done) and
    /// the channel checked as a publish does (missing: `plan_changed`; no
    /// view or history: `rejected`; no answer: `discord_unavailable`).
    pub(super) async fn search_run(
        &self,
        publish: &Publish,
        guide: Option<Guide>,
        stop: &watch::Receiver<bool>,
    ) -> Publish {
        let slots = match self.store.slots(publish.guide_id.clone()).await {
            Ok(slots) => slots,
            Err(error) => {
                kanade_log::log!(warn, "guide_read_failed", error = error.to_string());
                return self.stopped(publish, StopReason::Interrupted, None).await;
            }
        };
        let unknown: Vec<&PublishedSlot> = slots
            .iter()
            .filter(|slot| {
                slot.state == SlotState::Unknown
                    && slot.source.is_some()
                    && slot.channel_id == publish.channel_id
            })
            .collect();
        if unknown.is_empty() {
            return self.stopped(publish, StopReason::PlanChanged, None).await;
        }
        match self.inspect_channel(&publish.channel_id, &slots).await {
            None => {
                return self
                    .stopped(publish, StopReason::DiscordUnavailable, None)
                    .await;
            }
            Some(Inspection::ChannelMissing) => {
                return self.stopped(publish, StopReason::PlanChanged, None).await;
            }
            Some(Inspection::Channel { grants, .. }) if !(grants.view && grants.history) => {
                return self.stopped(publish, StopReason::Rejected, None).await;
            }
            Some(Inspection::Channel { .. }) => {}
        }
        let desired = match &guide {
            Some(guide) => self.render(guide).await.unwrap_or_default(),
            None => Vec::new(),
        };
        let mut unresolved = false;
        for (ordinal, slot) in (0u32..).zip(unknown) {
            if *stop.borrow() {
                return self.stopped(publish, StopReason::Interrupted, None).await;
            }
            if self.cancel_requested(publish).await {
                return self
                    .finish(publish, PublishState::Cancelled, None, None)
                    .await;
            }
            let source = slot.source.clone().unwrap_or_default();
            let op = PublishOp {
                ordinal,
                op: PlanOp::Create {
                    position: slot.position,
                    channel_id: slot.channel_id.clone(),
                    hash: slot.hash.clone(),
                },
                source: Some(source.clone()),
                outcome: OpOutcome::Pending,
                candidates: Vec::new(),
                at: self.now(),
            };
            if let Err(error) = self.store.record_op(publish.id.clone(), op).await {
                kanade_log::log!(warn, "guide_op_record_failed", error = error.to_string());
                return self.stopped(publish, StopReason::Interrupted, None).await;
            }
            let pending = Pending {
                publish_id: &publish.id,
                ordinal,
                position: slot.position,
                channel_id: &slot.channel_id,
                hash: &slot.hash,
                source: &source,
                uploads: slot.uploads.clone(),
            };
            match self.resolve(pending, &desired, &slots).await {
                Ok(posted) => unresolved |= !posted,
                Err(error) => {
                    // Left pending: the next start reconciles it.
                    kanade_log::log!(warn, "guide_op_settle_failed", error = error.to_string());
                    return publish.clone();
                }
            }
        }
        if unresolved {
            self.stopped(publish, StopReason::UnknownSlot, None).await
        } else {
            self.finish(publish, PublishState::Done, None, None).await
        }
    }

    /// On start (5.5), after the journal's own recovery made every
    /// unsettled claim indeterminate: queued runs end `interrupted` (the
    /// store), and each run a stopped process left `running` settles its
    /// pending op (a create by its bound post or the search; an edit or
    /// delete as unknown, since repeating either is safe) and ends
    /// `interrupted`. Runs never resume on their own.
    pub async fn recover(&self) {
        let runs = match self.store.interrupted_on_start(self.now()).await {
            Ok(runs) => runs,
            Err(error) => {
                kanade_log::log!(warn, "guide_recovery_failed", error = error.to_string());
                return;
            }
        };
        for (publish, ops) in runs {
            let guide = self
                .store
                .guide(publish.guide_id.clone())
                .await
                .ok()
                .flatten();
            let desired = match &guide {
                Some(guide) => self.render(guide).await.unwrap_or_default(),
                None => Vec::new(),
            };
            let known = self
                .store
                .slots(publish.guide_id.clone())
                .await
                .unwrap_or_default();
            for op in ops.iter().filter(|op| op.outcome == OpOutcome::Pending) {
                let settled = match (&op.op, op.source.as_deref()) {
                    (
                        PlanOp::Create {
                            position,
                            channel_id,
                            hash,
                        },
                        Some(source),
                    ) => {
                        let pending = Pending {
                            publish_id: &publish.id,
                            ordinal: op.ordinal,
                            position: *position,
                            channel_id,
                            hash,
                            source,
                            uploads: Vec::new(),
                        };
                        self.resolve(pending, &desired, &known).await.map(drop)
                    }
                    (PlanOp::Forget { .. }, _) => {
                        self.settle_unchanged(&publish, op.ordinal, OpOutcome::Skipped)
                            .await
                    }
                    _ => {
                        self.settle_unchanged(&publish, op.ordinal, OpOutcome::Unknown)
                            .await
                    }
                };
                if let Err(error) = settled {
                    kanade_log::log!(warn, "guide_recovery_failed", error = error.to_string());
                }
            }
            self.stopped(&publish, StopReason::Interrupted, None).await;
            kanade_log::log!(
                info,
                "guide_publish_recovered",
                publish = publish.id.as_str()
            );
        }
    }

    async fn settle_unchanged(
        &self,
        publish: &Publish,
        ordinal: u32,
        outcome: OpOutcome,
    ) -> Result<(), StoreError> {
        self.store
            .apply_op_result(
                publish.id.clone(),
                ordinal,
                outcome,
                SlotEffect::Unchanged,
                Vec::new(),
                self.now(),
            )
            .await
    }
}
