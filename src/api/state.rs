//! What admin read handlers need: one shared store (reads use reader
//! connections, never the writer), the schedule policy, the boss catalog and
//! the guild facts owned elsewhere (channels, personas, staff rule).

use std::{
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
        drafts::{DraftKind, DraftStatus, ProposalStore},
        history::{ChangeHistory, ChangeRef},
        members::{MemberProfile, MemberStore},
        schedule::{SchedulePolicy, ScheduleSnapshot},
        scheduler::{ScheduleStore, Scope, StoreError},
    },
};

pub type ReadFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// Object-safe reads over any store that implements the domain ports.
pub trait ReadStore: Send + Sync {
    fn snapshot(&self, scope: Scope) -> ReadFuture<'_, ScheduleSnapshot>;
    /// The history head: the week `version` the PWA sends back on edits (API-1).
    fn head(&self) -> ReadFuture<'_, ChangeRef>;
    fn members(&self) -> ReadFuture<'_, Vec<MemberProfile>>;
    fn member(&self, user_id: String) -> ReadFuture<'_, Option<MemberProfile>>;
    /// Live extractor proposals plus submitted member requests.
    fn inbox_count(&self) -> ReadFuture<'_, u64>;
}

impl<T> ReadStore for T
where
    T: ScheduleStore + ChangeHistory + MemberStore + ProposalStore + Send + Sync,
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
}

/// A channel the admin may pick (home channels, digest override).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelEntry {
    pub id: String,
    pub name: String,
    /// The extractor reads it.
    pub watched: bool,
}

/// Guild channels as the bot sees them (gateway cache in production).
pub trait ChannelList: Send + Sync {
    fn channels(&self) -> Vec<ChannelEntry>;
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
    pub policy: SchedulePolicy,
    pub catalog: Arc<BossTable>,
    pub channels: Arc<dyn ChannelList>,
    pub personas: Vec<PersonaOption>,
    pub access: Arc<GuildAccess>,
    /// Tracked `boss/knowledge/` (schema v2).
    pub knowledge_dir: Option<PathBuf>,
    /// For message links on posted cards.
    pub guild_id: Option<String>,
    pub clock: Clock,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("zone", &self.policy.zone())
            .field("personas", &self.personas.len())
            .field("knowledge_dir", &self.knowledge_dir)
            .finish_non_exhaustive()
    }
}

impl ApiState {
    pub fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }
}
