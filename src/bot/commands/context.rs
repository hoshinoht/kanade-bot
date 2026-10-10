//! What the retained commands work through. Writes go only through the
//! API's one scheduler [`Writer`] (so Discord and the portal share a single
//! serialised `SchedulerService`), reads through the shared [`ReadStore`];
//! guild facts and the optional rescan, chat and test-card ports are owned
//! elsewhere and wired by `serve`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use twilight_model::id::{Id, marker::InteractionMarker};

use super::access::Invoker;
use super::dispatch::CommandError;
use crate::api::admin::config::ConfigDesk;
use crate::api::rescan::RescanRunner;
use crate::api::state::{DeclineRetraction, GuildAccess, ReadStore};
use crate::api::write::{WriteContext, Writer};
use crate::bot::guild_cache::GuildCache;
use crate::bot::ids::{id_text, parse_id};
use crate::chat::pilot::AllowanceSnapshot;
use crate::domain::catalog::BossTable;
use crate::domain::history::{Actor, Origin, Surface};
use crate::domain::members::{GatewayMember, MemberProfile, MemberStore, Roster};
use crate::domain::schedule::{RunStatus, SchedulePolicy};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::MessageStyle;

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The guild's channels as the commands need them (the gateway cache in
/// production). Ids are Discord snowflakes as text.
pub trait GuildChannels: Send + Sync {
    /// v4 `is_watched`: listed, under a listed category, or a thread of one.
    fn is_watched(&self, channel_id: &str) -> bool;
    /// v4 `origin_ids`: a thread counts as its parent channel.
    fn origin(&self, channel_id: &str) -> String;
    /// The raw name (no `#`).
    fn name(&self, channel_id: &str) -> Option<String>;
    /// Watched text channels.
    fn watched(&self) -> Vec<String>;
    /// The bot may view and send there (unknown permissions count as yes).
    fn can_send(&self, channel_id: &str) -> bool;
}

impl GuildChannels for GuildCache {
    fn is_watched(&self, channel_id: &str) -> bool {
        parse_id(channel_id).is_some_and(|id| GuildCache::is_watched(self, id))
    }

    fn origin(&self, channel_id: &str) -> String {
        parse_id(channel_id).map_or_else(
            || channel_id.to_owned(),
            |id| id_text(GuildCache::origin(self, id).0),
        )
    }

    fn name(&self, channel_id: &str) -> Option<String> {
        self.channel_name(channel_id)
    }

    fn watched(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .channels()
            .into_iter()
            .filter(|channel| !channel.is_thread() && channel.is_messageable())
            .filter(|channel| GuildCache::is_watched(self, channel.id))
            .map(|channel| id_text(channel.id))
            .collect();
        ids.sort();
        ids
    }

    fn can_send(&self, channel_id: &str) -> bool {
        parse_id(channel_id).is_some_and(|id| GuildCache::can_send(self, id))
    }
}

/// Creates a member row the gateway has not written yet (v4 `upsert_member`
/// before `/pings` and `/style`). Never touches portal-owned fields.
pub trait MemberRows: Send + Sync {
    fn ensure(&self, member: GatewayMember) -> PortFuture<'_, Result<(), StoreError>>;
}

impl<T: MemberStore + Send + Sync> MemberRows for T {
    fn ensure(&self, member: GatewayMember) -> PortFuture<'_, Result<(), StoreError>> {
        Box::pin(async move {
            if self.load_member(&member.user_id).await?.is_none() {
                self.apply_gateway(member).await?;
            }
            Ok(())
        })
    }
}

/// The chat pilot's allowance, read for `/limits` (wired with the pilot).
pub trait ChatAllowance: Send + Sync {
    fn snapshot(&self) -> AllowanceSnapshot;
}

/// Which test message `/debug ping` posts (v4's kinds plus `digest`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TestKind {
    DayOf,
    Countdown60,
    Countdown15,
    Amend,
    Decline,
    Digest,
}

impl TestKind {
    pub const ALL: [Self; 6] = [
        Self::DayOf,
        Self::Countdown60,
        Self::Countdown15,
        Self::Amend,
        Self::Decline,
        Self::Digest,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DayOf => "day_of",
            Self::Countdown60 => "countdown_60",
            Self::Countdown15 => "countdown_15",
            Self::Amend => "amend",
            Self::Decline => "decline",
            Self::Digest => "digest",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// An in-memory run for a test card; never stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleRun {
    /// Canonical boss tokens.
    pub bosses: Vec<String>,
    pub at: DateTime<Utc>,
    /// User ids, in order.
    pub party: Vec<String>,
    /// Party members who answered ✅ / ❌; everyone else is waiting.
    pub yes: Vec<String>,
    pub no: Vec<String>,
    pub status: RunStatus,
}

/// What a test card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestSubject {
    /// A stored run (for `digest`: its boss week).
    Run(String),
    Sample(SampleRun),
    /// The current boss week's digest.
    Week,
}

/// One `/debug ping`. Channels are snowflakes as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PingRequest {
    pub subject: TestSubject,
    pub kind: TestKind,
    pub requested_by: String,
    /// `channel:`; else the configured test channel; else the default.
    pub channel: Option<String>,
    /// Where the command was used (samples' default).
    pub invoked_in: Option<String>,
    /// Overrides the live message style for this post only.
    pub style: Option<MessageStyle>,
    /// Rewrite the header now instead of using the seed.
    pub rewrite: bool,
}

impl PingRequest {
    /// v4's ping: a stored run, its default channel, the seed header.
    pub fn run(run_id: String, kind: TestKind, requested_by: String) -> Self {
        Self {
            subject: TestSubject::Run(run_id),
            kind,
            requested_by,
            channel: None,
            invoked_in: None,
            style: None,
            rewrite: false,
        }
    }
}

/// What posting a test card did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TestPosted {
    /// Posted and bound in this channel; its ✅/❌ drive the run's RSVPs.
    Posted { channel_id: String },
    /// Posted display-only (another channel, a sample or a digest): not
    /// registered for any run, not refreshed, no ✅/❌ seeded.
    Sandboxed { channel_id: String },
    /// The chosen channel cannot be reached.
    Unreachable,
    /// Discord did not confirm the post.
    Unconfirmed,
}

/// How a `header:rewrite` ping's header came out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderNote {
    pub rewritten: bool,
    /// `accepted`, or why the seed was used (`timeout`, `rejected (markup)`, …).
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestReport {
    pub posted: TestPosted,
    /// Set when a rewrite was asked for a kind that has a header.
    pub header: Option<HeaderNote>,
}

/// Which header `/debug header` rewrites.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderTrialKind {
    DayOf,
    Countdown,
    Digest,
}

impl HeaderTrialKind {
    pub const ALL: [Self; 3] = [Self::DayOf, Self::Countdown, Self::Digest];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::DayOf => "day_of",
            Self::Countdown => "countdown",
            Self::Digest => "digest",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// One `/debug header`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderRequest {
    pub kind: HeaderTrialKind,
    pub tries: u8,
    pub channel: Option<String>,
    pub invoked_in: Option<String>,
}

/// What `/debug header` did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderTrials {
    Posted {
        channel_id: String,
        accepted: usize,
    },
    /// No rewriter or persona is configured: nothing was tried.
    Disabled,
    Unreachable,
    /// Tried, but Discord did not confirm the post.
    Unconfirmed,
}

/// Test cards (v4 `debug_card` deliveries): posted with the `🧪 TEST — `
/// prefix, never touching the run's reminder rows, bound so reactions
/// route to the run when posted in its home channel, and deletable per
/// channel; plus header rewrite trials.
pub trait DebugCards: Send + Sync {
    fn ping(&self, request: PingRequest) -> PortFuture<'_, Result<TestReport, String>>;

    /// v4's ping (see [`PingRequest::run`]).
    fn post(
        &self,
        run_id: String,
        kind: TestKind,
        requested_by: String,
    ) -> PortFuture<'_, Result<TestPosted, String>> {
        let request = PingRequest::run(run_id, kind, requested_by);
        Box::pin(async move { self.ping(request).await.map(|report| report.posted) })
    }

    /// Delete this channel's test cards posted since `since`:
    /// `(deleted, failed)`.
    fn clear(
        &self,
        channel_id: String,
        since: DateTime<Utc>,
    ) -> PortFuture<'_, Result<(usize, usize), String>>;

    /// Rewrite a header `tries` times and post the results; stores nothing.
    fn headers(&self, request: HeaderRequest) -> PortFuture<'_, Result<HeaderTrials, String>>;
}

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Everything the retained commands share. Build it from the same `Arc`s
/// the API state holds.
pub struct CommandContext {
    pub store: Arc<dyn ReadStore>,
    pub writer: Arc<dyn Writer>,
    pub members: Arc<dyn MemberRows>,
    /// Run completion prompts, which their buttons answer.
    pub run_prompts: Arc<dyn crate::domain::completion::RunPromptStore>,
    pub policy: SchedulePolicy,
    pub catalog: Arc<BossTable>,
    pub channels: Arc<dyn GuildChannels>,
    /// The staff rule, admin role and chat pilot role.
    pub access: Arc<GuildAccess>,
    /// Live saved publication list and persona file snapshot.
    pub config: Option<Arc<ConfigDesk>>,
    /// `None` until the extractor's rescan runner is composed.
    pub rescans: Option<Arc<dyn RescanRunner>>,
    /// `None` until the chat pilot is composed.
    pub allowance: Option<Arc<dyn ChatAllowance>>,
    /// `None` until test cards have a delivery path.
    pub debug_cards: Option<Arc<dyn DebugCards>>,
    /// Live best-effort decline retraction; absent in offline composition.
    pub decline_retraction: Option<DeclineRetraction>,
    /// `/debug rewrite`'s manual header rewrite; absent offline.
    pub header_rewrite: Option<crate::api::state::HeaderRewritePort>,
    /// The bot's own name for access problems (v4 falls back to "the bot").
    pub bot_name: Option<String>,
    pub clock: Clock,
}

impl std::fmt::Debug for CommandContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandContext")
            .field("zone", &self.policy.zone())
            .field("rescans", &self.rescans.is_some())
            .field("allowance", &self.allowance.is_some())
            .field("debug_cards", &self.debug_cards.is_some())
            .finish_non_exhaustive()
    }
}

/// Store failures never reach members.
pub fn store_failed(error: StoreError) -> CommandError {
    CommandError::Internal(format!("store: {error}"))
}

impl CommandContext {
    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    pub async fn retract_decline(&self, run_id: String, user_id: String) {
        if let Some(retract) = &self.decline_retraction {
            retract(run_id, user_id, self.now()).await;
        }
    }

    /// v4 `is_admin`: the admin role only.
    pub fn is_admin(&self, invoker: &Invoker) -> bool {
        self.access.policy.is_admin(invoker)
    }

    /// Each command is its member's own change, made in Discord; the
    /// interaction id makes a redelivered write a no-op (`AlreadyApplied`).
    pub fn origin(&self, invoker: &Invoker, interaction: Id<InteractionMarker>) -> Origin {
        Origin::new(Actor::member(id_text(invoker.user_id)), Surface::Discord)
            .with_request_id(format!("discord:{}", interaction.get()))
    }

    /// Materialising is idempotent, so it carries no request id.
    pub fn plain_origin(&self, invoker: &Invoker) -> Origin {
        Origin::new(Actor::member(id_text(invoker.user_id)), Surface::Discord)
    }

    pub async fn profiles(&self) -> Result<Vec<MemberProfile>, CommandError> {
        self.store.members().await.map_err(store_failed)
    }

    /// Members plus watched channels, as the API's writes validate against.
    pub async fn write_context(&self) -> Result<(WriteContext, Vec<MemberProfile>), CommandError> {
        let profiles = self.profiles().await?;
        let mut directory = Roster::new();
        for profile in &profiles {
            directory.upsert(profile.member.clone());
        }
        for channel in self.channels.watched() {
            directory.watch(&channel);
        }
        Ok((
            WriteContext {
                policy: self.policy.clone(),
                directory,
            },
            profiles,
        ))
    }
}
