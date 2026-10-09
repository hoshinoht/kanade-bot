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
    bot::{
        commands::{AccessPolicy, Invoker},
        delivery::cards::{CardRecord, PostedCard, ReminderCardStore},
    },
    domain::{
        catalog::BossTable,
        drafts::{
            DraftKind, DraftStatus, LoadedDraft, ProposalInfo, ProposalStore, StoredDraft,
            StoredProposal,
        },
        history::{
            Actor, BlameIndex, BlameTarget, ChangeFilter, ChangeHistory, ChangeQuery, ChangeRecord,
            ChangeRef, HeldReminders, HistoryVerification, JournalHeld, changed_fields,
        },
        members::{MemberProfile, MemberStore, PortalEdit},
        model_log::{
            ChatFilter, ChatInteraction, ExtractionFilter, ExtractionLog, LogFacets, LogPage,
            MaskedTurn, ModelLogStore, RewriteFacets, RewriteFilter, RewriteLog, RewriteLogStore,
            WatchedMessage,
        },
        notify::{DeliveryJournal, WeeklyDigest},
        ownership::{OwnerRequest, OwnerRequestStatus, OwnerRequestStore},
        proposals::{ProposalCardStore, StoredCard},
        schedule::{SchedulePolicy, ScheduleSnapshot},
        scheduler::{ScheduleStore, Scope, StoreError},
        settings::{SettingsChange, SettingsChangeQuery, SettingsStore},
    },
    infrastructure::store::{
        auth_audit::{AuditFilter, AuditRow, AuthAuditStore},
        replays::{ReplayScope, ReplayStore, StoredReplay},
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
pub type HeaderRewritePort = Arc<
    dyn Fn(
            crate::bot::delivery::ManualRequest,
        ) -> Pin<Box<dyn Future<Output = crate::bot::delivery::ManualStart> + Send>>
        + Send
        + Sync,
>;

/// Live governor snapshots for the Limits page. `None` means model serving is
/// not composed; allowance rows still remain useful.
pub type ModelLimits = Arc<
    dyn Fn(DateTime<Utc>) -> Vec<crate::infrastructure::llm::governor::GroupSnapshot> + Send + Sync,
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
    /// Closed proposals and member requests, rows only (no operations).
    fn closed_inbox(&self) -> ReadFuture<'_, Vec<ClosedItem>>;
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
    /// Whether the chain still holds `anchor` (a backup manifest's head).
    fn contains_anchor(&self, anchor: ChangeRef) -> ReadFuture<'_, bool>;
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
    fn rewrite_logs(&self, filter: RewriteFilter) -> ReadFuture<'_, LogPage<RewriteLog>>;
    fn rewrite_log(&self, id: String) -> ReadFuture<'_, Option<RewriteLog>>;
    fn rewrite_log_facets(&self) -> ReadFuture<'_, RewriteFacets>;
    /// Every weekly digest card, active or retired (the Config page's last post).
    fn digests(&self) -> ReadFuture<'_, Vec<WeeklyDigest>>;
    /// Recorded Config section saves, newest first.
    fn settings_changes(&self, query: SettingsChangeQuery) -> ReadFuture<'_, Vec<SettingsChange>>;
    /// Record a History entry that writes no settings row (a Limits window
    /// clear, section `limits`), with the keyed request's replay if any, in
    /// one transaction.
    fn record_settings_change(
        &self,
        change: SettingsChange,
        replay: Option<StoredReplay>,
    ) -> ReadFuture<'_, ()>;
    /// The live `Idempotency-Key` replay of `(scope, actor, key)` at `now`.
    fn replay(
        &self,
        scope: ReplayScope,
        actor: String,
        key: String,
        now: DateTime<Utc>,
    ) -> ReadFuture<'_, Option<StoredReplay>>;
    /// Record a keyed request's answer (see [`ReplayStore::put_replay`]).
    fn put_replay(&self, replay: StoredReplay) -> ReadFuture<'_, ()>;
    /// The stored reminder card record (heading) under a send's native
    /// dedupe key; read only.
    fn card_record(&self, dedupe_key: String) -> ReadFuture<'_, Option<CardRecord>>;
    /// The bound reminder cards naming `run_id` (and their records); read only.
    fn posted_cards(&self, run_id: String) -> ReadFuture<'_, Vec<PostedCard>>;
    /// A page of the sign-in audit log, newest first.
    fn audit_page(&self, filter: AuditFilter) -> ReadFuture<'_, Vec<AuditRow>>;
    /// Ownership requests (see [`OwnerRequestStore`]).
    fn create_owner_request(&self, request: OwnerRequest) -> ReadFuture<'_, ()>;
    fn owner_request(&self, id: String) -> ReadFuture<'_, Option<OwnerRequest>>;
    fn open_owner_requests(&self) -> ReadFuture<'_, Vec<OwnerRequest>>;
    fn close_owner_request(
        &self,
        id: String,
        status: OwnerRequestStatus,
        decided_by: String,
        at: DateTime<Utc>,
    ) -> ReadFuture<'_, bool>;
    fn set_owner_request_message(
        &self,
        id: String,
        channel_id: String,
        message_id: String,
    ) -> ReadFuture<'_, ()>;
    fn unsettled_owner_requests(&self) -> ReadFuture<'_, Vec<OwnerRequest>>;
    fn settle_owner_request_message(&self, id: String) -> ReadFuture<'_, ()>;
}

/// A closed Inbox item: a proposal (with its stored facts) or a member
/// request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedItem {
    pub draft: StoredDraft,
    /// `None` for member requests.
    pub proposal: Option<ProposalInfo>,
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
        + RewriteLogStore
        + DeliveryJournal
        + SettingsStore
        + ReminderCardStore
        + ReplayStore
        + AuthAuditStore
        + OwnerRequestStore
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

    fn closed_inbox(&self) -> ReadFuture<'_, Vec<ClosedItem>> {
        Box::pin(async move {
            let mut items: Vec<ClosedItem> = self
                .list_proposals(false)
                .await?
                .into_iter()
                .filter(|stored| !stored.draft.status.is_live())
                .map(|stored| ClosedItem {
                    draft: stored.draft,
                    proposal: Some(stored.info),
                })
                .collect();
            items.extend(
                self.list_drafts(None)
                    .await?
                    .into_iter()
                    .filter(|draft| draft.kind == DraftKind::Request && !draft.status.is_live())
                    .map(|draft| ClosedItem {
                        draft,
                        proposal: None,
                    }),
            );
            Ok(items)
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

    fn contains_anchor(&self, anchor: ChangeRef) -> ReadFuture<'_, bool> {
        Box::pin(async move { ChangeHistory::contains_anchor(self, &anchor).await })
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

    fn rewrite_logs(&self, filter: RewriteFilter) -> ReadFuture<'_, LogPage<RewriteLog>> {
        Box::pin(async move { self.list_rewrites(&filter).await })
    }

    fn rewrite_log(&self, id: String) -> ReadFuture<'_, Option<RewriteLog>> {
        Box::pin(async move { self.load_rewrite(&id).await })
    }

    fn rewrite_log_facets(&self) -> ReadFuture<'_, RewriteFacets> {
        Box::pin(self.rewrite_facets())
    }

    fn digests(&self) -> ReadFuture<'_, Vec<WeeklyDigest>> {
        Box::pin(async move {
            self.load_digests()
                .await
                .map(|log| log.digests)
                .map_err(|error| StoreError::Backend(error.to_string()))
        })
    }

    fn settings_changes(&self, query: SettingsChangeQuery) -> ReadFuture<'_, Vec<SettingsChange>> {
        Box::pin(SettingsStore::settings_changes(self, query))
    }

    fn record_settings_change(
        &self,
        change: SettingsChange,
        replay: Option<StoredReplay>,
    ) -> ReadFuture<'_, ()> {
        Box::pin(async move {
            match replay {
                Some(replay) => {
                    self.put_settings_rows_replayed(Vec::new(), Some(change), replay)
                        .await
                }
                None => self
                    .put_settings_rows_recorded(Vec::new(), change)
                    .await
                    .map(drop),
            }
        })
    }

    fn replay(
        &self,
        scope: ReplayScope,
        actor: String,
        key: String,
        now: DateTime<Utc>,
    ) -> ReadFuture<'_, Option<StoredReplay>> {
        Box::pin(async move { ReplayStore::replay(self, scope, &actor, &key, now).await })
    }

    fn put_replay(&self, replay: StoredReplay) -> ReadFuture<'_, ()> {
        Box::pin(ReplayStore::put_replay(self, replay))
    }

    fn card_record(&self, dedupe_key: String) -> ReadFuture<'_, Option<CardRecord>> {
        Box::pin(async move { ReminderCardStore::card_record(self, &dedupe_key).await })
    }

    fn posted_cards(&self, run_id: String) -> ReadFuture<'_, Vec<PostedCard>> {
        Box::pin(async move { ReminderCardStore::posted_cards(self, &run_id).await })
    }

    fn audit_page(&self, filter: AuditFilter) -> ReadFuture<'_, Vec<AuditRow>> {
        Box::pin(async move { AuthAuditStore::audit_page(self, &filter).await })
    }

    fn create_owner_request(&self, request: OwnerRequest) -> ReadFuture<'_, ()> {
        Box::pin(OwnerRequestStore::create_owner_request(self, request))
    }

    fn owner_request(&self, id: String) -> ReadFuture<'_, Option<OwnerRequest>> {
        Box::pin(async move { OwnerRequestStore::owner_request(self, &id).await })
    }

    fn open_owner_requests(&self) -> ReadFuture<'_, Vec<OwnerRequest>> {
        Box::pin(OwnerRequestStore::open_owner_requests(self))
    }

    fn close_owner_request(
        &self,
        id: String,
        status: OwnerRequestStatus,
        decided_by: String,
        at: DateTime<Utc>,
    ) -> ReadFuture<'_, bool> {
        Box::pin(async move {
            OwnerRequestStore::close_owner_request(self, &id, status, &decided_by, at).await
        })
    }

    fn set_owner_request_message(
        &self,
        id: String,
        channel_id: String,
        message_id: String,
    ) -> ReadFuture<'_, ()> {
        Box::pin(async move {
            OwnerRequestStore::set_owner_request_message(self, &id, &channel_id, &message_id).await
        })
    }

    fn unsettled_owner_requests(&self) -> ReadFuture<'_, Vec<OwnerRequest>> {
        Box::pin(OwnerRequestStore::unsettled_owner_requests(self))
    }

    fn settle_owner_request_message(&self, id: String) -> ReadFuture<'_, ()> {
        Box::pin(async move { OwnerRequestStore::settle_owner_request_message(self, &id).await })
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

    /// A member's gateway portrait; `None` (the monogram) when the gateway
    /// has not shown one.
    fn member_avatar(&self, _user_id: &str) -> Option<super::avatars::AvatarRef> {
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
    /// The profile's one-line voice; empty when it has none.
    pub voice: String,
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

/// Where `kanade backup` writes (History checkpoints read it, never write).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackupDir {
    /// `KANADE_BACKUP_DIR`; `None` lists no backups.
    pub dir: Option<PathBuf>,
    /// The open store's schema version: older backups are flagged.
    pub schema_version: i64,
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
    /// Live model-governor groups for the Limits page.
    pub model_limits: Option<ModelLimits>,
    /// Bounded idempotency memory for Limits and manual digest operations.
    pub limits: Arc<super::admin::limits::LimitsDesk>,
    /// The shared CardDesk refresh, attached only after Discord composition.
    pub proposal_refresh: Option<ProposalCardRefresh>,
    /// The shared delivery retraction, attached only after Discord composition.
    pub decline_retraction: Option<DeclineRetraction>,
    /// The delivery-owned manual digest port, absent while Discord is offline.
    pub digest_post: Option<DigestPost>,
    /// The delivery-owned manual header rewrite, absent while Discord is offline.
    pub header_rewrite: Option<HeaderRewritePort>,
    pub backups: BackupDir,
    /// Member and admin portraits; `None` serves monograms only.
    pub avatars: Option<Arc<super::avatars::AvatarCache>>,
    /// Change hints for open admin pages (`GET /api/admin/events`), fed by
    /// the store's write hook.
    pub events: Arc<super::events::Hub>,
    /// Difficulty emojis for redesigned card previews, as the bot listed
    /// them at startup; empty (written labels) until Discord composes.
    pub marks: crate::bot::delivery::cards::DifficultyMarks,
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
