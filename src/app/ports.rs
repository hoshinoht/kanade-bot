//! What the bot side hands the web side: post-commit card effects, the
//! delivery-owned digest and header rewrite, the guide publisher and the
//! live model limits. Each is absent while its owner is not composed.

use std::{future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};

/// The live Discord card desk's post-commit proposal-card refresh. It is
/// absent in offline API composition, where a decision still commits normally.
pub type ProposalCardRefresh =
    Arc<dyn Fn(Vec<String>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The live delivery side's best-effort, post-commit decline deletion. It is
/// absent in offline API composition; durable pending state is recovered by
/// the delivery tick in that case.
pub type DeclineRetraction = Arc<
    dyn Fn(String, String, DateTime<Utc>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

/// A digest post always crosses the delivery boundary; the API only asks for
/// it and never obtains a Discord transport.
pub type DigestPost = Arc<
    dyn Fn(DigestPostRequest) -> Pin<Box<dyn Future<Output = DigestPostResult> + Send>>
        + Send
        + Sync,
>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestPostRequest {
    pub week_start: DateTime<Utc>,
    pub channel_id: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DigestPostResult {
    Completed,
    NewerWeekAlreadyPosted,
    Unavailable,
}

/// The delivery-owned manual header rewrite (`POST /api/admin/headers/rewrite`,
/// `/debug rewrite`): plans and queues one run and answers at once.
pub type HeaderRewritePort =
    Arc<dyn Fn(ManualRequest) -> Pin<Box<dyn Future<Output = ManualStart> + Send>> + Send + Sync>;

/// The delivery-owned guide publisher (`modules.guides`); the API never
/// holds a transport. Runs are queued in the store
/// ([`kanade_domain::guides::GuideStore::request_publish`]) and Discord's
/// availability is [`ChannelList::connected`](super::guild::ChannelList::connected).
pub trait GuidePort: Send + Sync {
    /// Read the channel for a plan (4.4): which of `slots` are gone, its
    /// newest message and the bot's grants, within
    /// [`kanade_domain::guides::INSPECT_BUDGET`]; `None` when Discord did
    /// not answer in time (the plan is then "Unchecked").
    fn inspect(
        &self,
        channel_id: String,
        slots: Vec<kanade_domain::guides::PublishedSlot>,
    ) -> Pin<Box<dyn Future<Output = Option<kanade_domain::guides::Inspection>> + Send + '_>>;
    /// Wake the single-flight worker to claim queued runs (of any guide).
    fn wake(&self);
    /// A worker runs to claim queued runs; without one, publish, unpublish
    /// and "Search again" refuse `discord_offline` and nothing is queued.
    fn publishing(&self) -> bool;
}

/// The guide port while the bot is not composed (the `web` component alone):
/// guides stay editable, plans read "Unchecked" and nothing publishes.
pub struct OfflineGuides;

impl GuidePort for OfflineGuides {
    fn inspect(
        &self,
        _channel_id: String,
        _slots: Vec<kanade_domain::guides::PublishedSlot>,
    ) -> Pin<Box<dyn Future<Output = Option<kanade_domain::guides::Inspection>> + Send + '_>> {
        Box::pin(std::future::ready(None))
    }

    fn wake(&self) {}

    fn publishing(&self) -> bool {
        false
    }
}

/// Live governor snapshots for the Limits page. `None` means model serving is
/// not composed; allowance rows still remain useful.
pub type ModelLimits = Arc<
    dyn Fn(DateTime<Utc>) -> Vec<crate::infrastructure::llm::governor::GroupSnapshot> + Send + Sync,
>;

/// Who asked for a run and where its summary goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualRequest {
    /// Recorded with each override: `member:<id>` or the admin session's actor.
    pub actor: String,
    /// A channel to post the summary in when the run ends (`/debug rewrite`).
    pub report_to: Option<String>,
}

/// The answer to a trigger, given before any model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualStart {
    /// Queued: this many headers will be rewritten.
    Started(usize),
    /// Another manual run is queued or running.
    Running,
    /// No rewrite model or persona.
    Disabled,
    /// Nothing posted this boss week has a header to rewrite.
    Nothing,
    /// The store could not be read, or the worker has stopped.
    Unavailable,
}
