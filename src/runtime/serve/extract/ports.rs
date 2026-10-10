//! The pipeline's `Guild` and `Proposer` over the live cache, roster and
//! store, and the stale-message cache the feed writes through.

use std::sync::Arc;

use crate::{
    api::{auth::Clock, write::ApiClock},
    bot::{extract_feed::StaleCache, guild_cache::GuildCache, ids::parse_id, roster::LiveRoster},
    domain::{
        catalog::BossTable,
        ids::RandomIds,
        schedule::SchedulePolicy,
        scheduler::{ProposalRequest, ProposalResult, Proposed, SchedulerService, SupersedeScope},
    },
    extract::pipeline::{Guild, IncomingMessage, Outbox, Proposer},
    infrastructure::{
        llm::{LlmProvider, identity::Member},
        store::SqliteStore,
    },
};

use super::status::ExtractionStatus;
use crate::extract::pipeline::Extractor;

pub struct LiveGuild {
    pub status: Arc<ExtractionStatus>,
    pub cache: Arc<GuildCache>,
    pub roster: Arc<LiveRoster>,
    pub bosses: Arc<BossTable>,
}

impl Guild for LiveGuild {
    fn extraction_enabled(&self) -> bool {
        self.status.enabled()
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        parse_id(channel_id).is_some_and(|id| self.cache.is_watched(id))
    }

    fn members(&self) -> Vec<Member> {
        self.roster
            .profiles()
            .into_iter()
            .filter(|profile| !profile.member.is_bot)
            .map(|profile| Member {
                display_name: profile
                    .member
                    .display_name
                    .clone()
                    .unwrap_or_else(|| profile.member.user_id.clone()),
                user_id: profile.member.user_id,
                nickname: profile.member.nickname,
                aliases: profile.aliases,
            })
            .collect()
    }

    fn has_role(&self, user_id: &str) -> bool {
        self.roster
            .profile(user_id)
            .is_some_and(|profile| profile.member.has_role && !profile.member.is_bot)
    }

    fn bosses(&self) -> Arc<BossTable> {
        Arc::clone(&self.bosses)
    }

    fn channel_name(&self, channel_id: &str) -> String {
        self.cache
            .channel_name(channel_id)
            .unwrap_or_else(|| channel_id.to_owned())
    }
}

/// The scheduler's proposal service, one short-lived service per call.
pub struct StoreProposer {
    pub store: Arc<SqliteStore>,
    pub clock: Clock,
    pub policy: SchedulePolicy,
    pub directory: Arc<LiveRoster>,
    pub run_ends: crate::domain::completion::RunEndsSource,
}

impl StoreProposer {
    fn service(&self) -> SchedulerService<Arc<SqliteStore>, RandomIds, ApiClock> {
        SchedulerService::new(
            Arc::clone(&self.store),
            RandomIds,
            ApiClock(Arc::clone(&self.clock)),
        )
        .with_attendance(self.policy.attendance)
        .with_run_ends(self.run_ends.clone())
    }
}

impl Proposer for StoreProposer {
    async fn supersede(&self, scope: SupersedeScope<'_>) -> ProposalResult<Vec<String>> {
        self.service().supersede_proposals(scope).await
    }

    async fn propose(&self, request: ProposalRequest) -> ProposalResult<Proposed> {
        self.service()
            .propose(request, &self.policy, &*self.directory)
            .await
    }
}

/// Stale messages go straight to the messages cache (members' messages in
/// watched channels only, as for live ones).
pub struct ExtractorCache<S, P, X, O>(pub Arc<Extractor<S, P, X, O>>);

impl<P, X, O> StaleCache for ExtractorCache<SqliteStore, P, X, O>
where
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
    Extractor<SqliteStore, P, X, O>: Send + Sync,
{
    async fn cache(&self, message: IncomingMessage) {
        // A failed write loses nothing a rescan's backfill cannot restore.
        let _ = self.0.store_message(&message).await;
    }
}
