//! What admin read handlers need: one shared store (reads use reader
//! connections, never the writer), the schedule policy, the boss catalog and
//! the guild facts owned elsewhere (channels, personas, staff rule).

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, RwLock},
};

use chrono::{DateTime, Utc};
use twilight_model::id::{Id, marker::UserMarker};

use super::auth::Clock;
use crate::{
    bot::commands::{AccessPolicy, Invoker},
    domain::{
        catalog::BossTable,
        drafts::{DraftKind, DraftStatus, LoadedDraft, ProposalStore, StoredProposal},
        history::{
            Actor, Blame, BlameIndex, BlameTarget, ChangeFilter, ChangeHistory, ChangeQuery,
            ChangeRecord, ChangeRef, HeldReminders, HistoryVerification, JournalHeld, blame,
            changed_fields,
        },
        members::{MemberProfile, MemberStore, PortalEdit},
        model_log::{
            ChatFilter, ChatInteraction, ExtractionFilter, ExtractionLog, LogFacets, LogPage,
            MaskedTurn, ModelLogStore, WatchedMessage,
        },
        notify::DeliveryJournal,
        proposals::{ProposalCardStore, StoredCard},
        schedule::{SchedulePolicy, ScheduleSnapshot},
        scheduler::{ScheduleStore, Scope, StoreError},
    },
};

pub type ReadFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

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

/// Object-safe reads over any store that implements the domain ports.
pub trait ReadStore: Send + Sync {
    fn snapshot(&self, scope: Scope) -> ReadFuture<'_, ScheduleSnapshot>;
    /// The history head: the week `version` the PWA sends back on edits (API-1).
    fn head(&self) -> ReadFuture<'_, ChangeRef>;
    fn members(&self) -> ReadFuture<'_, Vec<MemberProfile>>;
    fn member(&self, user_id: String) -> ReadFuture<'_, Option<MemberProfile>>;
    /// Live extractor proposals plus submitted member requests.
    fn inbox_count(&self) -> ReadFuture<'_, u64>;
    /// Live proposals, oldest first.
    fn live_proposals(&self) -> ReadFuture<'_, Vec<StoredProposal>>;
    /// Submitted member requests with their operations, oldest first.
    fn submitted_requests(&self) -> ReadFuture<'_, Vec<LoadedDraft>>;
    /// Any draft (admin, request or proposal) with its operations.
    fn draft(&self, id: String) -> ReadFuture<'_, Option<LoadedDraft>>;
    fn cards(&self, proposal_ids: Vec<String>) -> ReadFuture<'_, Vec<StoredCard>>;
    /// A channel's cached messages created at or after `since`.
    fn messages(
        &self,
        channel_id: String,
        since: DateTime<Utc>,
    ) -> ReadFuture<'_, Vec<WatchedMessage>>;
    /// Each field of `target` any record set, with the last record's seq.
    fn last_changes(&self, target: BlameTarget) -> ReadFuture<'_, BTreeMap<String, u64>>;
    /// The last record at or before `version` that set `field` of `target`:
    /// what a client that read at `version` saw (history is append-only).
    fn seen_at(
        &self,
        target: BlameTarget,
        field: String,
        version: u64,
    ) -> ReadFuture<'_, Option<u64>>;
    /// The weekly timing a recorded request created (idempotent create replays).
    /// The change recorded for the actor's request id (an idempotent replay).
    fn recorded_change(
        &self,
        actor: Actor,
        request_id: String,
    ) -> ReadFuture<'_, Option<ChangeRecord>>;
    /// Portal member edits bypass the scheduler (members are not history rows).
    fn edit_member(
        &self,
        user_id: String,
        edit: PortalEdit,
    ) -> ReadFuture<'_, Option<MemberProfile>>;
    /// Newest first, older than `before`, genesis never included.
    fn history_page(
        &self,
        filter: ChangeFilter,
        before: Option<u64>,
        limit: usize,
    ) -> ReadFuture<'_, HistorySlice>;
    fn history_total(&self, filter: ChangeFilter) -> ReadFuture<'_, u64>;
    fn change(&self, seq: u64) -> ReadFuture<'_, Option<ChangeRecord>>;
    fn verify_history(&self) -> ReadFuture<'_, HistoryVerification>;
    fn blame(&self, target: BlameTarget) -> ReadFuture<'_, Option<Blame>>;
    /// Reminders unresolved delivery attempts hold (rollbacks keep them).
    fn held_reminders(&self) -> ReadFuture<'_, BTreeSet<String>>;
    fn extraction_logs(&self, filter: ExtractionFilter) -> ReadFuture<'_, LogPage<ExtractionLog>>;
    fn extraction_log(&self, id: String) -> ReadFuture<'_, Option<ExtractionLog>>;
    /// Cached messages by id, in the order asked; pruned ones are left out.
    fn messages_by_id(&self, ids: Vec<String>) -> ReadFuture<'_, Vec<WatchedMessage>>;
    fn extraction_log_facets(&self) -> ReadFuture<'_, LogFacets>;
    fn chat_logs(&self, filter: ChatFilter) -> ReadFuture<'_, LogPage<ChatInteraction>>;
    fn chat_log(&self, id: String) -> ReadFuture<'_, Option<ChatInteraction>>;
    fn chat_log_facets(&self) -> ReadFuture<'_, LogFacets>;
    /// The Model view stored with a masked chat turn.
    fn masked_chat(&self, id: String) -> ReadFuture<'_, Option<MaskedTurn>>;
}

/// One history page: records and whether older ones exist.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistorySlice {
    pub records: Vec<ChangeRecord>,
    /// The seq to pass as `before` for the next page.
    pub next_before: Option<u64>,
}

impl<T> ReadStore for T
where
    T: ScheduleStore
        + ChangeHistory
        + BlameIndex
        + MemberStore
        + ProposalStore
        + ProposalCardStore
        + ModelLogStore
        + DeliveryJournal
        + Send
        + Sync,
{
    fn snapshot(&self, scope: Scope) -> ReadFuture<'_, ScheduleSnapshot> {
        Box::pin(async move { self.load(&scope).await })
    }

    fn head(&self) -> ReadFuture<'_, ChangeRef> {
        Box::pin(self.history_head())
    }

    fn members(&self) -> ReadFuture<'_, Vec<MemberProfile>> {
        Box::pin(self.list_members())
    }

    fn member(&self, user_id: String) -> ReadFuture<'_, Option<MemberProfile>> {
        Box::pin(async move { self.load_member(&user_id).await })
    }

    fn inbox_count(&self) -> ReadFuture<'_, u64> {
        Box::pin(async move {
            let proposals = self.list_proposals(true).await?.len();
            let requests = self
                .list_drafts(Some(DraftStatus::Submitted))
                .await?
                .iter()
                .filter(|draft| draft.kind == DraftKind::Request)
                .count();
            Ok((proposals + requests) as u64)
        })
    }

    fn live_proposals(&self) -> ReadFuture<'_, Vec<StoredProposal>> {
        Box::pin(self.list_proposals(true))
    }

    fn submitted_requests(&self) -> ReadFuture<'_, Vec<LoadedDraft>> {
        Box::pin(async move {
            let mut requests = Vec::new();
            for draft in self.list_drafts(Some(DraftStatus::Submitted)).await? {
                if draft.kind == DraftKind::Request
                    && let Some(loaded) = self.load_draft(&draft.id).await?
                {
                    requests.push(loaded);
                }
            }
            Ok(requests)
        })
    }

    fn draft(&self, id: String) -> ReadFuture<'_, Option<LoadedDraft>> {
        Box::pin(async move { self.load_draft(&id).await })
    }

    fn cards(&self, proposal_ids: Vec<String>) -> ReadFuture<'_, Vec<StoredCard>> {
        Box::pin(async move { self.load_cards(&proposal_ids).await })
    }

    fn messages(
        &self,
        channel_id: String,
        since: DateTime<Utc>,
    ) -> ReadFuture<'_, Vec<WatchedMessage>> {
        Box::pin(async move { self.channel_messages(&channel_id, since, false).await })
    }

    fn last_changes(&self, target: BlameTarget) -> ReadFuture<'_, BTreeMap<String, u64>> {
        Box::pin(async move { BlameIndex::last_changes(self, &target).await })
    }

    fn seen_at(
        &self,
        target: BlameTarget,
        field: String,
        version: u64,
    ) -> ReadFuture<'_, Option<u64>> {
        Box::pin(async move {
            let key = (target, field);
            let mut query = ChangeQuery::new(ChangeFilter::All);
            query.newest_first = true;
            query.cursor = Some(version.saturating_add(1));
            loop {
                let page = self.list_changes(&query).await?;
                if let Some(record) = page
                    .records
                    .iter()
                    .find(|record| changed_fields(record).contains(&key))
                {
                    return Ok(Some(record.seq));
                }
                match page.next_cursor {
                    Some(cursor) => query.cursor = Some(cursor),
                    None => return Ok(None),
                }
            }
        })
    }

    fn recorded_change(
        &self,
        actor: Actor,
        request_id: String,
    ) -> ReadFuture<'_, Option<ChangeRecord>> {
        Box::pin(async move {
            let Some(recorded) = self.recorded_request(&actor, &request_id).await? else {
                return Ok(None);
            };
            self.load_change(recorded.committed.seq).await
        })
    }

    fn edit_member(
        &self,
        user_id: String,
        edit: PortalEdit,
    ) -> ReadFuture<'_, Option<MemberProfile>> {
        Box::pin(async move { self.apply_portal(&user_id, edit).await })
    }

    fn history_page(
        &self,
        filter: ChangeFilter,
        before: Option<u64>,
        limit: usize,
    ) -> ReadFuture<'_, HistorySlice> {
        Box::pin(async move {
            let mut query = ChangeQuery::new(filter);
            query.newest_first = true;
            query.cursor = before;
            query.limit = limit;
            let page = self.list_changes(&query).await?;
            let records: Vec<ChangeRecord> = page
                .records
                .into_iter()
                .filter(|record| record.seq > 0)
                .collect();
            // The next page may hold only genesis, which is never listed.
            let next_before = match page.next_cursor {
                Some(cursor) => {
                    query.cursor = Some(cursor);
                    query.limit = 1;
                    let older = self.list_changes(&query).await?;
                    older
                        .records
                        .iter()
                        .any(|record| record.seq > 0)
                        .then_some(cursor)
                }
                None => None,
            };
            Ok(HistorySlice {
                records,
                next_before,
            })
        })
    }

    fn history_total(&self, filter: ChangeFilter) -> ReadFuture<'_, u64> {
        Box::pin(async move { ChangeHistory::count_changes(self, &filter).await })
    }

    fn change(&self, seq: u64) -> ReadFuture<'_, Option<ChangeRecord>> {
        Box::pin(async move { Ok(self.load_change(seq).await?.filter(|record| record.seq > 0)) })
    }

    fn verify_history(&self) -> ReadFuture<'_, HistoryVerification> {
        Box::pin(ChangeHistory::verify_history(self))
    }

    fn blame(&self, target: BlameTarget) -> ReadFuture<'_, Option<Blame>> {
        Box::pin(async move { blame(self, &target).await })
    }

    fn held_reminders(&self) -> ReadFuture<'_, BTreeSet<String>> {
        Box::pin(async move { JournalHeld(self).held_reminders().await })
    }

    fn extraction_logs(&self, filter: ExtractionFilter) -> ReadFuture<'_, LogPage<ExtractionLog>> {
        Box::pin(async move { self.list_extractions(&filter).await })
    }

    fn extraction_log(&self, id: String) -> ReadFuture<'_, Option<ExtractionLog>> {
        Box::pin(async move { self.load_extraction(&id).await })
    }

    fn messages_by_id(&self, ids: Vec<String>) -> ReadFuture<'_, Vec<WatchedMessage>> {
        Box::pin(async move { self.messages_by_ids(&ids).await })
    }

    fn extraction_log_facets(&self) -> ReadFuture<'_, LogFacets> {
        Box::pin(self.extraction_facets())
    }

    fn chat_logs(&self, filter: ChatFilter) -> ReadFuture<'_, LogPage<ChatInteraction>> {
        Box::pin(async move { self.list_chats(&filter).await })
    }

    fn chat_log(&self, id: String) -> ReadFuture<'_, Option<ChatInteraction>> {
        Box::pin(async move { self.load_chat(&id).await })
    }

    fn chat_log_facets(&self) -> ReadFuture<'_, LogFacets> {
        Box::pin(self.chat_facets())
    }

    fn masked_chat(&self, id: String) -> ReadFuture<'_, Option<MaskedTurn>> {
        Box::pin(async move { self.load_masked_chat(&id).await })
    }
}

/// A channel the admin may pick (home channels, digest override).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelEntry {
    pub id: String,
    pub name: String,
    /// The extractor reads it.
    pub watched: bool,
}

/// A guild role for id→name display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleEntry {
    pub id: String,
    pub name: String,
    /// `0xRRGGBB`; `None` when the role has no colour.
    pub color: Option<u32>,
}

/// Guild channels as the bot sees them (gateway cache in production).
pub trait ChannelList: Send + Sync {
    fn channels(&self) -> Vec<ChannelEntry>;

    /// Offline and test lists know no roles.
    fn roles(&self) -> Vec<RoleEntry> {
        Vec::new()
    }

    /// The bot's own Discord user id, once the gateway said `READY`.
    fn bot_user_id(&self) -> Option<String> {
        None
    }

    /// The bot's live display name, once the gateway said `READY`.
    fn bot_name(&self) -> Option<String> {
        None
    }

    /// The guild is loaded, so [`ChannelList::grants`] can answer.
    fn connected(&self) -> bool {
        false
    }

    /// What the bot may do in one channel; `None` while its permissions are
    /// unknown (v4 counts unknown as allowed).
    fn grants(&self, _id: &str) -> Option<ChannelGrants> {
        None
    }
}

/// The bot's effective permissions in a channel, as the access report needs
/// them (v4 `access_report`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelGrants {
    pub view: bool,
    pub send: bool,
    pub history: bool,
    pub embed: bool,
    pub react: bool,
    pub manage_messages: bool,
}

impl ChannelGrants {
    /// v4's reading of unknown permissions.
    pub const UNKNOWN: Self = Self {
        view: true,
        send: true,
        history: true,
        embed: true,
        react: true,
        manage_messages: true,
    };
}

impl std::fmt::Debug for dyn ChannelList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelList")
    }
}

/// A fixed list: offline use and tests.
pub struct StaticChannels(pub Vec<ChannelEntry>);

impl ChannelList for StaticChannels {
    fn channels(&self) -> Vec<ChannelEntry> {
        self.0.clone()
    }
}

/// A reply-style profile members may choose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersonaOption {
    pub key: String,
    pub name: String,
}

/// The bot's staff rule plus the facts it reads that are not member rows:
/// the guild owner (from `GuildAvailable`) and the chat pilot role.
pub struct GuildAccess {
    pub policy: AccessPolicy,
    pub pilot_role: Option<String>,
    owner: RwLock<Option<Id<UserMarker>>>,
}

impl GuildAccess {
    pub fn new(policy: AccessPolicy, pilot_role: Option<String>) -> Self {
        Self {
            policy,
            pilot_role,
            owner: RwLock::new(None),
        }
    }

    pub fn set_owner(&self, owner: Option<Id<UserMarker>>) {
        *self
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = owner;
    }

    pub fn owner(&self) -> Option<Id<UserMarker>> {
        *self
            .owner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The member as the staff rule sees them; bots never qualify.
    pub fn invoker(profile: &MemberProfile) -> Option<Invoker> {
        let user_id = profile
            .member
            .user_id
            .parse::<u64>()
            .ok()
            .and_then(Id::new_checked)?;
        (!profile.member.is_bot).then(|| Invoker {
            user_id,
            roles: profile
                .roles
                .iter()
                .filter_map(|role| role.parse::<u64>().ok().and_then(Id::new_checked))
                .collect(),
            is_guild_admin: profile.is_guild_admin,
        })
    }

    pub fn is_staff(&self, profile: &MemberProfile) -> bool {
        Self::invoker(profile).is_some_and(|invoker| self.policy.is_staff(&invoker, self.owner()))
    }

    /// `staff`, `pilot` (chatbot pilot role) or `none`.
    pub fn access(&self, profile: &MemberProfile) -> &'static str {
        if self.is_staff(profile) {
            "staff"
        } else if self
            .pilot_role
            .as_ref()
            .is_some_and(|role| profile.roles.contains(role))
        {
            "pilot"
        } else {
            "none"
        }
    }
}

/// Everything the admin read routes compose.
pub struct ApiState {
    pub store: Arc<dyn ReadStore>,
    /// The one scheduler writer (a mutex inside); reads never go through it.
    pub writer: Arc<dyn super::write::Writer>,
    pub policy: SchedulePolicy,
    pub catalog: Arc<BossTable>,
    pub channels: Arc<dyn ChannelList>,
    pub access: Arc<GuildAccess>,
    /// Tracked `boss/knowledge/` (schema v2).
    pub knowledge_dir: Option<PathBuf>,
    /// For message links on posted cards.
    pub guild_id: Option<String>,
    pub clock: Clock,
    /// Rescan jobs; `None` until the extractor is composed (503).
    pub rescans: Option<Arc<super::rescan::RescanDesk>>,
    /// Runtime settings (A9); `None` answers `unavailable`.
    pub config: Option<Arc<super::admin::config::ConfigDesk>>,
    /// The chat pilot's Limits view and status (for A8); empty until serve
    /// starts chat.
    pub chat: Option<Arc<crate::chat::driver::ChatHandle>>,
    /// The shared CardDesk refresh, attached only after Discord composition.
    pub proposal_refresh: Option<ProposalCardRefresh>,
    /// The shared delivery retraction, attached only after Discord composition.
    pub decline_retraction: Option<DeclineRetraction>,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("zone", &self.policy.zone())
            .field("personas", &self.profile_options().len())
            .field("knowledge_dir", &self.knowledge_dir)
            .finish_non_exhaustive()
    }
}

impl ApiState {
    /// Public reply-profile choices from the config desk. Missing settings
    /// fail closed rather than inferring visibility from readable files.
    pub fn profile_options(&self) -> Vec<PersonaOption> {
        self.config
            .as_ref()
            .map(|desk| desk.profile_choices().options)
            .unwrap_or_default()
    }

    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }

    /// Refresh the proposal cards touched by a committed inbox decision.
    pub async fn refresh_proposals(&self, proposal_ids: Vec<String>) {
        if !proposal_ids.is_empty()
            && let Some(refresh) = &self.proposal_refresh
        {
            refresh(proposal_ids).await;
        }
    }

    /// Delete a bound decline notice after its RSVP commit. Delivery failures
    /// are intentionally contained: S2 keeps the durable pending retraction
    /// for the next recovery tick.
    pub async fn retract_decline(&self, run_id: String, user_id: String) {
        if let Some(retract) = &self.decline_retraction {
            retract(run_id, user_id, self.now()).await;
        }
    }
}
