//! The worker loop: wait for the gateway, reconcile runs a stopped process
//! left behind, then claim and run queued runs one at a time until stopped.
//! A run re-reads the channel, re-plans from the saved guide and runs only
//! if the plan hash is the one the admin confirmed.

use std::sync::Arc;

use tokio::sync::watch;

use super::Publisher;
use super::execute::Run;
use crate::bot::transport::DiscordTransport;
use crate::domain::guides::{
    Blocked, Guide, GuideStore, Inspection, Publish, PublishKind, PublishState, RenderedSlot,
    StopReason, plan, plan_unpublish, render,
};
use crate::domain::notify::{DeliveryJournal, Lease};
use crate::runtime::logging;

impl<S, T> Publisher<S, T>
where
    S: GuideStore + DeliveryJournal + Send + Sync + 'static,
    T: DiscordTransport + 'static,
{
    /// The worker task: runs until `stop` turns true (or its sender goes).
    /// Nothing is read or sent before the gateway is ready (`ready`), since
    /// the searches need the bot's own id.
    pub async fn run(&self, mut ready: watch::Receiver<bool>, mut stop: watch::Receiver<bool>) {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stop| *stop) => return,
            ready = ready.wait_for(|ready| *ready) => if ready.is_err() { return },
        }
        self.recover().await;
        loop {
            while !*stop.borrow() && self.run_next(&stop).await.is_some() {}
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => break,
                () = self.wake.notified() => {}
            }
        }
    }

    /// Claim the oldest queued run and carry it out: the run as it ended
    /// (still `running` when a stop cut a create short; the next start
    /// reconciles it), or `None` with nothing queued.
    pub async fn run_next(&self, stop: &watch::Receiver<bool>) -> Option<Publish> {
        let publish = match self.store.claim_next(self.now()).await {
            Ok(publish) => publish?,
            Err(error) => {
                logging::log!(warn, "guide_claim_failed", error = error.to_string());
                return None;
            }
        };
        let ended = self.execute(publish, stop).await;
        logging::log!(
            info,
            "guide_publish_ended",
            publish = ended.id.as_str(),
            kind = ended.kind.as_str(),
            state = ended.state.as_str(),
            stop_reason = ended.stop_reason.map(|reason| reason.as_str()),
        );
        Some(ended)
    }

    async fn execute(&self, publish: Publish, stop: &watch::Receiver<bool>) -> Publish {
        let guide = match self.store.guide(publish.guide_id.clone()).await {
            Ok(guide) => guide,
            Err(error) => {
                logging::log!(warn, "guide_read_failed", error = error.to_string());
                return self.stopped(&publish, StopReason::Interrupted, None).await;
            }
        };
        if publish.kind == PublishKind::Search {
            return self.search_run(&publish, guide, stop).await;
        }
        let Some(guide) = guide else {
            return self.stopped(&publish, StopReason::PlanChanged, None).await;
        };
        let desired = if publish.kind == PublishKind::Publish {
            let same = guide.version == publish.version
                && guide.channel_id.as_deref() == Some(publish.channel_id.as_str());
            match self.render(&guide).await.filter(|_| same) {
                Some(desired) => desired,
                None => return self.stopped(&publish, StopReason::PlanChanged, None).await,
            }
        } else {
            Vec::new()
        };
        let slots = match self.store.slots(guide.id.clone()).await {
            Ok(slots) => slots,
            Err(error) => {
                logging::log!(warn, "guide_read_failed", error = error.to_string());
                return self.stopped(&publish, StopReason::Interrupted, None).await;
            }
        };
        let (plan, anchor) = if publish.kind == PublishKind::Publish {
            // The worker always re-reads the channel before acting.
            let Some(inspection) = self.inspect_channel(&publish.channel_id, &slots).await else {
                return self
                    .stopped(&publish, StopReason::DiscordUnavailable, None)
                    .await;
            };
            let anchor = match &inspection {
                Inspection::Channel {
                    newest_message_id, ..
                } => newest_message_id.clone(),
                Inspection::ChannelMissing => None,
            };
            let plan = plan(
                &slots,
                &desired,
                &publish.channel_id,
                publish.delete_old,
                Some(&inspection),
            );
            (plan, anchor)
        } else {
            (plan_unpublish(&slots, &publish.channel_id), None)
        };
        // The confirmed plan or nothing: a moved plan sends nothing.
        if plan.hash != publish.plan_hash || !publish.confirmed.covers(&plan.needs) {
            return self.stopped(&publish, StopReason::PlanChanged, None).await;
        }
        let blocked = match &plan.blocked {
            None => None,
            Some(Blocked::UnknownSlot { .. }) => Some(StopReason::UnknownSlot),
            Some(Blocked::ChannelMissing) => Some(StopReason::PlanChanged),
            Some(Blocked::Grants(_)) => Some(StopReason::Rejected),
        };
        if let Some(reason) = blocked {
            return self.stopped(&publish, reason, None).await;
        }
        Run::new(self, &publish, &desired, slots, anchor)
            .ops(plan.ops, stop)
            .await
    }

    /// Render the saved draft on the blocking pool (art reads block);
    /// `None` when it no longer renders.
    pub(super) async fn render(&self, guide: &Guide) -> Option<Vec<RenderedSlot>> {
        let blocks = guide.blocks.clone();
        let catalog = Arc::clone(&self.catalog);
        let sources = Arc::clone(&self.sources);
        tokio::task::spawn_blocking(move || render(&blocks, &catalog, &*sources))
            .await
            .ok()?
            .ok()
            .map(|rendered| rendered.slots)
    }

    /// Whether an admin asked the run to stop (read between ops).
    pub(super) async fn cancel_requested(&self, publish: &Publish) -> bool {
        matches!(
            self.store.publish(publish.id.clone()).await,
            Ok(Some((run, _))) if run.cancel_requested_at.is_some()
        )
    }

    pub(super) async fn stopped(
        &self,
        publish: &Publish,
        reason: StopReason,
        lease: Option<&Lease>,
    ) -> Publish {
        self.finish(publish, PublishState::Stopped, Some(reason), lease)
            .await
    }

    /// End the run (and its journal lease, best effort: an unended lease is
    /// orphaned by the next start's recovery).
    pub(super) async fn finish(
        &self,
        publish: &Publish,
        state: PublishState,
        reason: Option<StopReason>,
        lease: Option<&Lease>,
    ) -> Publish {
        let now = self.now();
        if let Some(lease) = lease {
            let _ = self.store.end_lease(lease, now).await;
        }
        match self
            .store
            .finish(publish.id.clone(), state, reason, now)
            .await
        {
            Ok(ended) => ended,
            Err(error) => {
                logging::log!(
                    warn,
                    "guide_finish_failed",
                    publish = publish.id.as_str(),
                    error = error.to_string(),
                );
                publish.clone()
            }
        }
    }
}
