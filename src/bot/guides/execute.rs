//! One confirmed plan, op by op (plan 5.1, 5.2, 5.4): each op is recorded
//! `pending` before its effect and settled with its slot effect in one
//! store write. A create that is not bound, an edit or delete that is not
//! confirmed, a cancel or a stop ends the run: nothing later is sent.

use tokio::sync::watch;
use uuid::Uuid;

use super::inspect::deadline;
use super::reconcile::Found;
use super::{GUIDE_PUBLISH_OPERATION, Publisher, payload};
use crate::bot::delivery::{Executor, LogAlerts, SendOutcome};
use crate::bot::ids::{id_text, parse_id};
use crate::bot::transport::{DiscordTransport, Outcome, Presence, RejectionKind, SILENT};
use crate::domain::guides::{
    GuideStore, OpOutcome, PlanOp, Publish, PublishOp, PublishState, PublishedSlot, RenderedSlot,
    SlotEffect, SlotState, StopReason,
};
use crate::domain::notify::{
    DeliveryJournal, EffectKind, IntentContent, Lease, NotificationIntent,
};
use crate::runtime::logging;

/// What follows an op.
pub(super) enum Next {
    Go,
    Stop(StopReason),
    /// Stopping mid-create, or a store write failed after Discord acted:
    /// the op stays `pending` and the run `running` for the next start's
    /// reconciliation.
    Halt,
}

pub(super) struct Run<'a, S, T> {
    publisher: &'a Publisher<S, T>,
    publish: &'a Publish,
    desired: &'a [RenderedSlot],
    /// The slot map as this run left it so far.
    slots: Vec<PublishedSlot>,
    /// Begun on the first create.
    lease: Option<Lease>,
    /// The channel's newest message before the run: any create is newer.
    anchor: Option<String>,
}

/// The larger of two snowflakes in decimal text.
pub(super) fn newest(a: Option<String>, b: Option<&str>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) if (b.len(), b) > (a.len(), a.as_str()) => Some(b.to_owned()),
        (Some(a), _) => Some(a),
        (None, b) => b.map(str::to_owned),
    }
}

/// A create in `channel_id` is newer than every posted slot there.
pub(super) fn after(
    slots: &[PublishedSlot],
    channel_id: &str,
    anchor: Option<String>,
) -> Option<String> {
    slots
        .iter()
        .filter(|slot| slot.channel_id == channel_id && slot.state == SlotState::Posted)
        .fold(anchor, |newest_yet, slot| {
            newest(newest_yet, slot.message_id.as_deref())
        })
}

impl<'a, S, T> Run<'a, S, T>
where
    S: GuideStore + DeliveryJournal + Send + Sync + 'static,
    T: DiscordTransport + 'static,
{
    pub(super) fn new(
        publisher: &'a Publisher<S, T>,
        publish: &'a Publish,
        desired: &'a [RenderedSlot],
        slots: Vec<PublishedSlot>,
        anchor: Option<String>,
    ) -> Self {
        Self {
            publisher,
            publish,
            desired,
            slots,
            lease: None,
            anchor,
        }
    }

    pub(super) async fn ops(mut self, ops: Vec<PlanOp>, stop: &watch::Receiver<bool>) -> Publish {
        let publisher = self.publisher;
        for (ordinal, op) in (0u32..).zip(ops) {
            if *stop.borrow() {
                return self
                    .end(PublishState::Stopped, Some(StopReason::Interrupted))
                    .await;
            }
            if publisher.cancel_requested(self.publish).await {
                return self.end(PublishState::Cancelled, None).await;
            }
            let source = matches!(op, PlanOp::Create { .. })
                .then(|| format!("guide-post:{}", Uuid::new_v4()));
            let pending = PublishOp {
                ordinal,
                op: op.clone(),
                source: source.clone(),
                outcome: OpOutcome::Pending,
                candidates: Vec::new(),
                at: publisher.now(),
            };
            if let Err(error) = publisher
                .store
                .record_op(self.publish.id.clone(), pending)
                .await
            {
                logging::log!(warn, "guide_op_record_failed", error = error.to_string());
                return self
                    .end(PublishState::Stopped, Some(StopReason::Interrupted))
                    .await;
            }
            let next = match op {
                PlanOp::Edit {
                    position,
                    channel_id,
                    message_id,
                    ..
                } => self.edit(ordinal, position, channel_id, &message_id).await,
                PlanOp::Create {
                    position,
                    channel_id,
                    ..
                } => {
                    let source = source.unwrap_or_default();
                    self.create(ordinal, position, channel_id, &source, stop)
                        .await
                }
                PlanOp::Delete {
                    position,
                    channel_id,
                    message_id,
                } => {
                    self.delete(ordinal, position, channel_id, &message_id)
                        .await
                }
                PlanOp::Forget {
                    position,
                    channel_id,
                } => {
                    let effect = SlotEffect::Remove {
                        channel_id,
                        position,
                    };
                    self.settle(ordinal, OpOutcome::Done, effect, Vec::new(), Next::Go)
                        .await
                }
            };
            match next {
                Next::Go => {}
                Next::Stop(reason) => return self.end(PublishState::Stopped, Some(reason)).await,
                Next::Halt => return self.publish.clone(),
            }
        }
        self.end(PublishState::Done, None).await
    }

    async fn end(&self, state: PublishState, reason: Option<StopReason>) -> Publish {
        self.publisher
            .finish(self.publish, state, reason, self.lease.as_ref())
            .await
    }

    /// Settle op `ordinal` and apply `effect` to the local map; `next` when
    /// stored, else [`Next::Halt`].
    async fn settle(
        &mut self,
        ordinal: u32,
        outcome: OpOutcome,
        effect: SlotEffect,
        candidates: Vec<String>,
        next: Next,
    ) -> Next {
        let stored = self
            .publisher
            .store
            .apply_op_result(
                self.publish.id.clone(),
                ordinal,
                outcome,
                effect.clone(),
                candidates,
                self.publisher.now(),
            )
            .await;
        if let Err(error) = stored {
            logging::log!(
                warn,
                "guide_op_settle_failed",
                publish = self.publish.id.as_str(),
                ordinal,
                error = error.to_string(),
            );
            return Next::Halt;
        }
        match effect {
            SlotEffect::Unchanged => {}
            SlotEffect::Put(row) => {
                self.slots.retain(|slot| {
                    (slot.channel_id.as_str(), slot.position)
                        != (row.channel_id.as_str(), row.position)
                });
                self.slots.push(row);
            }
            SlotEffect::Remove {
                channel_id,
                position,
            } => self.slots.retain(|slot| {
                (slot.channel_id.as_str(), slot.position) != (channel_id.as_str(), position)
            }),
        }
        next
    }

    fn row(
        &self,
        position: u32,
        channel_id: String,
        message_id: Option<String>,
        state: SlotState,
        slot: &RenderedSlot,
        source: Option<String>,
    ) -> PublishedSlot {
        PublishedSlot {
            position,
            channel_id,
            message_id,
            hash: slot.hash.clone(),
            uploads: slot.manifest(),
            state,
            source,
            publish_id: self.publish.id.clone(),
            updated_at: self.publisher.now(),
        }
    }

    async fn lease(&mut self) -> Option<&Lease> {
        if self.lease.is_none() {
            let publisher = self.publisher;
            match publisher
                .store
                .begin_lease(
                    &publisher.instance_id,
                    GUIDE_PUBLISH_OPERATION,
                    publisher.now(),
                )
                .await
            {
                Ok(lease) => self.lease = Some(lease),
                Err(error) => {
                    logging::log!(warn, "guide_lease_failed", error = error.to_string());
                }
            }
        }
        self.lease.as_ref()
    }

    /// 5.1: claimed by its source, sent `SILENT` with its files, bound; an
    /// unconfirmed create is searched for, never sent again.
    async fn create(
        &mut self,
        ordinal: u32,
        position: u32,
        channel_id: String,
        source: &str,
        stop: &watch::Receiver<bool>,
    ) -> Next {
        let desired = self.desired;
        let Some(slot) = desired.get(position as usize) else {
            return self
                .settle(
                    ordinal,
                    OpOutcome::Skipped,
                    SlotEffect::Unchanged,
                    Vec::new(),
                    Next::Stop(StopReason::PlanChanged),
                )
                .await;
        };
        let publisher = self.publisher;
        let intent = NotificationIntent {
            effect: EffectKind::GuideSlot,
            effect_context: vec![self.publish.id.clone(), position.to_string()],
            channel_id: channel_id.clone(),
            targets: Vec::new(),
            mentions: Vec::new(),
            content: IntentContent::Plain,
            warnings: Vec::new(),
        };
        let message = payload::create(slot);
        let sent = match self.lease().await {
            Some(lease) => {
                let executor = Executor {
                    journal: &*publisher.store,
                    transport: &*publisher.transport,
                    alerts: &LogAlerts,
                    throttle: &publisher.throttle,
                    lease,
                };
                executor
                    .execute_source(&intent, &message, source, 0, SILENT, publisher.now())
                    .await
            }
            None => Ok(SendOutcome::Unavailable("no journal lease".into())),
        };
        let (outcome, next) = match sent {
            Ok(SendOutcome::Bound(id)) => {
                let row = self.row(
                    position,
                    channel_id,
                    Some(id_text(id)),
                    SlotState::Posted,
                    slot,
                    None,
                );
                return self
                    .settle(
                        ordinal,
                        OpOutcome::Done,
                        SlotEffect::Put(row),
                        Vec::new(),
                        Next::Go,
                    )
                    .await;
            }
            Ok(SendOutcome::Rejected(_)) => (OpOutcome::Rejected, StopReason::Rejected),
            Ok(SendOutcome::Uncertain | SendOutcome::Suppressed) => {
                return self
                    .unconfirmed(ordinal, position, channel_id, source, slot, stop)
                    .await;
            }
            Err(failure) if failure.maybe_delivered => {
                return self
                    .unconfirmed(ordinal, position, channel_id, source, slot, stop)
                    .await;
            }
            // Never reached Discord (released, not claimable, a journal
            // write refused before the send): nothing landed.
            Ok(SendOutcome::Released(_) | SendOutcome::Unavailable(_) | SendOutcome::Failed(_))
            | Err(_) => (OpOutcome::Skipped, StopReason::DiscordUnavailable),
        };
        self.settle(
            ordinal,
            outcome,
            SlotEffect::Unchanged,
            Vec::new(),
            Next::Stop(next),
        )
        .await
    }

    /// An ambiguous create: held forever by the journal; found by its
    /// content in the channel, else unknown, and the run stops.
    async fn unconfirmed(
        &mut self,
        ordinal: u32,
        position: u32,
        channel_id: String,
        source: &str,
        slot: &RenderedSlot,
        stop: &watch::Receiver<bool>,
    ) -> Next {
        if *stop.borrow() {
            // The search's reads would be cut too: the next start searches.
            return Next::Halt;
        }
        let after = after(&self.slots, &channel_id, self.anchor.clone());
        match self
            .publisher
            .search(&channel_id, Some(slot), after.as_deref())
            .await
        {
            Found::One(message_id) => {
                let row = self.row(
                    position,
                    channel_id,
                    Some(message_id),
                    SlotState::Posted,
                    slot,
                    None,
                );
                self.settle(
                    ordinal,
                    OpOutcome::Recovered,
                    SlotEffect::Put(row),
                    Vec::new(),
                    Next::Go,
                )
                .await
            }
            Found::Not(candidates) => {
                let row = self.row(
                    position,
                    channel_id,
                    None,
                    SlotState::Unknown,
                    slot,
                    Some(source.to_owned()),
                );
                self.settle(
                    ordinal,
                    OpOutcome::Unknown,
                    SlotEffect::Put(row),
                    candidates,
                    Next::Stop(StopReason::UnknownSlot),
                )
                .await
            }
        }
    }

    /// 5.2: idempotent, so an unconfirmed edit is tried once more.
    async fn edit(
        &mut self,
        ordinal: u32,
        position: u32,
        channel_id: String,
        message_id: &str,
    ) -> Next {
        let ids = parse_id(&channel_id).zip(parse_id(message_id));
        let desired = self.desired;
        let (Some(slot), Some((channel, message))) = (desired.get(position as usize), ids) else {
            return self
                .settle(
                    ordinal,
                    OpOutcome::Rejected,
                    SlotEffect::Unchanged,
                    Vec::new(),
                    Next::Stop(StopReason::Rejected),
                )
                .await;
        };
        let publisher = self.publisher;
        let edit = payload::edit(slot);
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(publisher.pauses.retry).await;
            }
            match deadline(publisher.transport.edit_message(channel, message, &edit)).await {
                Outcome::Delivered(()) => {
                    let row = self.row(
                        position,
                        channel_id,
                        Some(message_id.to_owned()),
                        SlotState::Posted,
                        slot,
                        None,
                    );
                    return self
                        .settle(
                            ordinal,
                            OpOutcome::Done,
                            SlotEffect::Put(row),
                            Vec::new(),
                            Next::Go,
                        )
                        .await;
                }
                Outcome::DefinitelyRejected(
                    RejectionKind::UnknownMessage | RejectionKind::UnknownChannel,
                ) => {
                    // Deleted by hand: the next plan decides tail or middle.
                    let row = self.gone(position, channel_id, message_id);
                    return self
                        .settle(
                            ordinal,
                            OpOutcome::Gone,
                            SlotEffect::Put(row),
                            Vec::new(),
                            Next::Stop(StopReason::PlanChanged),
                        )
                        .await;
                }
                Outcome::Ambiguous(_)
                | Outcome::DefinitelyRejected(
                    RejectionKind::NotSent | RejectionKind::RateLimited,
                ) => {}
                Outcome::DefinitelyRejected(_) => {
                    return self
                        .settle(
                            ordinal,
                            OpOutcome::Rejected,
                            SlotEffect::Unchanged,
                            Vec::new(),
                            Next::Stop(StopReason::Rejected),
                        )
                        .await;
                }
            }
        }
        self.settle(
            ordinal,
            OpOutcome::Unknown,
            SlotEffect::Unchanged,
            Vec::new(),
            Next::Stop(StopReason::EditUnconfirmed),
        )
        .await
    }

    /// The slot as it is, now gone (its message id kept).
    fn gone(&self, position: u32, channel_id: String, message_id: &str) -> PublishedSlot {
        let known = self
            .slots
            .iter()
            .find(|slot| slot.channel_id == channel_id && slot.position == position);
        PublishedSlot {
            position,
            message_id: Some(message_id.to_owned()),
            hash: known.map(|slot| slot.hash.clone()).unwrap_or_default(),
            uploads: known.map(|slot| slot.uploads.clone()).unwrap_or_default(),
            state: SlotState::Gone,
            source: None,
            publish_id: self.publish.id.clone(),
            updated_at: self.publisher.now(),
            channel_id,
        }
    }

    /// 5.2: a delete is confirmed by Discord or by the message's absence;
    /// unconfirmed, the run stops before any create.
    async fn delete(
        &mut self,
        ordinal: u32,
        position: u32,
        channel_id: String,
        message_id: &str,
    ) -> Next {
        let Some((channel, message)) = parse_id(&channel_id).zip(parse_id(message_id)) else {
            return self
                .settle(
                    ordinal,
                    OpOutcome::Rejected,
                    SlotEffect::Unchanged,
                    Vec::new(),
                    Next::Stop(StopReason::Rejected),
                )
                .await;
        };
        let publisher = self.publisher;
        let removed = SlotEffect::Remove {
            channel_id: channel_id.clone(),
            position,
        };
        let mut not_sent = false;
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(publisher.pauses.retry).await;
            }
            match deadline(publisher.transport.delete_message(channel, message)).await {
                Outcome::Delivered(()) => {
                    return self
                        .settle(ordinal, OpOutcome::Done, removed, Vec::new(), Next::Go)
                        .await;
                }
                Outcome::DefinitelyRejected(
                    RejectionKind::UnknownMessage | RejectionKind::UnknownChannel,
                ) => {
                    return self
                        .settle(ordinal, OpOutcome::Gone, removed, Vec::new(), Next::Go)
                        .await;
                }
                Outcome::DefinitelyRejected(
                    RejectionKind::NotSent | RejectionKind::RateLimited,
                ) => {
                    not_sent = true;
                }
                Outcome::DefinitelyRejected(_) => {
                    return self
                        .settle(
                            ordinal,
                            OpOutcome::Rejected,
                            SlotEffect::Unchanged,
                            Vec::new(),
                            Next::Stop(StopReason::Rejected),
                        )
                        .await;
                }
                Outcome::Ambiguous(_) => {
                    not_sent = false;
                    match deadline(publisher.transport.message_presence(channel, message)).await {
                        Outcome::Delivered(Presence::Absent) => {
                            return self
                                .settle(ordinal, OpOutcome::Done, removed, Vec::new(), Next::Go)
                                .await;
                        }
                        // Still there: delete once more.
                        Outcome::Delivered(Presence::Present) => {}
                        _ => break,
                    }
                }
            }
        }
        let (outcome, reason) = if not_sent {
            (OpOutcome::Skipped, StopReason::DiscordUnavailable)
        } else {
            (OpOutcome::Unknown, StopReason::DeleteUnconfirmed)
        };
        self.settle(
            ordinal,
            outcome,
            SlotEffect::Unchanged,
            Vec::new(),
            Next::Stop(reason),
        )
        .await
    }
}
