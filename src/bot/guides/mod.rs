//! The guide publisher's Discord side (module `guides`, plan 4-5): one
//! single-flight worker claims queued runs from the store (oldest first,
//! any guide), re-reads the channel, re-plans from the saved guide and
//! runs the confirmed plan only if its hash still matches, one op at a time.
//!
//! - Creates are journalled by a durable `guide-post:<op uuid>` source
//!   ([`EffectKind::GuideSlot`](kanade_domain::notify::EffectKind)) through
//!   the delivery [`Executor`](crate::bot::delivery::Executor), `SILENT`,
//!   mentioning nobody, with the slot's files. An unconfirmed create is
//!   never sent again: the channel's recent history is searched for it
//!   (`reconcile.rs`); one match is the slot, none or several leave it
//!   unknown and stop the run.
//! - Edits (text and embeds only; the files stay) and deletes are not
//!   journalled: repeating them is safe. An edit's 404 marks the slot gone
//!   and stops the run `plan_changed`; an unconfirmed delete stops the run
//!   before any create (replace only after a confirmed deletion).
//! - The slot map changes only with an op's result, in one store write, so
//!   a cancel (between ops), a stop (between ops) or a crash (mid-op,
//!   reconciled on the next start) leaves it exact.
//! - Calls are sequential with a 30 s deadline each ([`CALL_DEADLINE`]);
//!   Twilight's own rate limiter paces them, and nothing is retried beyond
//!   the single edit/delete retry after [`Pauses::retry`].

mod execute;
mod inspect;
mod payload;
mod reconcile;
mod worker;

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use crate::app::clock::Clock;
use crate::app::ports::GuidePort;
use crate::bot::chardata::SelfId;
use crate::bot::delivery::AlertThrottle;
use crate::discord::transport::DiscordTransport;
use kanade_domain::catalog::BossTable;
use kanade_domain::guides::{GuideSources, GuideStore, Inspection, PublishedSlot};
use kanade_domain::notify::DeliveryJournal;

pub use inspect::{ChannelView, GuideChannels};

/// The journal lease operation every publish run's creates claim under.
pub const GUIDE_PUBLISH_OPERATION: &str = "guide_publish";
/// The deadline of each Discord call the worker makes itself. A create runs
/// inside the executor's claim-send-bind sequence, which must never be cut
/// mid-journal, so it relies on the transport's own 30 s call deadline.
pub const CALL_DEADLINE: Duration = Duration::from_secs(30);
/// How many of the channel's latest messages the channel check reads.
pub const INSPECT_PAGE: u16 = 100;
/// How many of the channel's latest messages an unconfirmed create's search
/// reads, and how often (as share cards, `chardata/posts.rs`).
pub const SEARCH_DEPTH: u16 = 50;
pub const SEARCHES: usize = 2;

/// Waits between the searches for an unconfirmed create and before the
/// one retry of an edit or delete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pauses {
    pub search: Duration,
    pub retry: Duration,
}

impl Default for Pauses {
    fn default() -> Self {
        Self {
            search: Duration::from_secs(2),
            retry: Duration::from_secs(1),
        }
    }
}

/// What the publisher reads and calls.
pub struct PublisherParts<S, T> {
    pub store: Arc<S>,
    pub transport: Arc<T>,
    /// The guild cache in serve: the target channel and the bot's grants.
    pub channels: Arc<dyn GuideChannels>,
    pub catalog: Arc<BossTable>,
    /// The same adapter the API preview renders with, so slot hashes agree.
    pub sources: Arc<dyn GuideSources + Send + Sync>,
    /// The bot's own user id (authors the messages a search may match).
    pub self_id: SelfId,
    pub clock: Clock,
    /// Names this process's journal leases.
    pub instance_id: String,
    pub pauses: Pauses,
}

/// The single-flight publish worker and the API's [`GuidePort`].
pub struct Publisher<S, T> {
    store: Arc<S>,
    transport: Arc<T>,
    channels: Arc<dyn GuideChannels>,
    catalog: Arc<BossTable>,
    sources: Arc<dyn GuideSources + Send + Sync>,
    self_id: SelfId,
    clock: Clock,
    instance_id: String,
    pauses: Pauses,
    throttle: AlertThrottle,
    wake: Notify,
}

impl<S, T> Publisher<S, T> {
    pub fn new(parts: PublisherParts<S, T>) -> Self {
        let PublisherParts {
            store,
            transport,
            channels,
            catalog,
            sources,
            self_id,
            clock,
            instance_id,
            pauses,
        } = parts;
        Self {
            store,
            transport,
            channels,
            catalog,
            sources,
            self_id,
            clock,
            instance_id,
            pauses,
            throttle: AlertThrottle::new(),
            wake: Notify::new(),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }
}

impl<S, T> GuidePort for Publisher<S, T>
where
    S: GuideStore + DeliveryJournal + Send + Sync + 'static,
    T: DiscordTransport + 'static,
{
    fn inspect(
        &self,
        channel_id: String,
        slots: Vec<PublishedSlot>,
    ) -> Pin<Box<dyn Future<Output = Option<Inspection>> + Send + '_>> {
        Box::pin(async move { self.inspect_channel(&channel_id, &slots).await })
    }

    fn wake(&self) {
        // A stored permit: a wake before the worker waits is not lost.
        self.wake.notify_one();
    }

    fn publishing(&self) -> bool {
        true
    }
}
