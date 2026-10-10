//! Who may use the admin portal: the bot's staff rule
//! (`AccessPolicy::is_staff`: Administrator, guild owner or the admin role),
//! read from the bot's own guild member data, never from OAuth scopes.

use std::{future::Future, pin::Pin, sync::Arc};

use twilight_model::id::{Id, marker::UserMarker};

use crate::bot::commands::{AccessPolicy, Invoker};

pub type GateFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StaffCheck {
    Staff,
    /// Not in the guild, or in it without a staff grant.
    NotStaff,
    /// Member data is unavailable (bot disconnected); callers fail closed.
    Unavailable,
}

pub trait StaffGate: Send + Sync {
    fn check<'a>(&'a self, discord_user_id: &'a str) -> GateFuture<'a, StaffCheck>;
}

/// The guild the bot serves, as its member cache sees it.
pub trait GuildMembers: Send + Sync {
    /// `Ok(None)` when the user is not a member; `Err` when unknown.
    fn member(&self, user_id: Id<UserMarker>) -> GateFuture<'_, Result<Option<Invoker>, ()>>;
    fn owner_id(&self) -> Option<Id<UserMarker>>;
}

/// [`StaffGate`] applying the bot's staff rule to guild member data.
pub struct GuildStaffGate {
    policy: AccessPolicy,
    members: Arc<dyn GuildMembers>,
}

impl GuildStaffGate {
    pub fn new(policy: AccessPolicy, members: Arc<dyn GuildMembers>) -> Self {
        Self { policy, members }
    }
}

/// [`GuildMembers`] over the persisted member rows (the bot's gateway view:
/// roles and computed Administrator) and the owner the gateway reported.
pub struct StoreGuildMembers<S> {
    store: S,
    access: Arc<crate::api::state::GuildAccess>,
}

impl<S> StoreGuildMembers<S> {
    pub fn new(store: S, access: Arc<crate::api::state::GuildAccess>) -> Self {
        Self { store, access }
    }
}

impl<S: crate::domain::members::MemberStore + Send + Sync> GuildMembers for StoreGuildMembers<S> {
    fn member(&self, user_id: Id<UserMarker>) -> GateFuture<'_, Result<Option<Invoker>, ()>> {
        Box::pin(async move {
            let profile = self
                .store
                .load_member(&user_id.get().to_string())
                .await
                .map_err(|_| ())?;
            // Departures clear the stored roles, so a former admin no longer qualifies.
            Ok(profile
                .as_ref()
                .and_then(crate::api::state::GuildAccess::invoker))
        })
    }

    fn owner_id(&self) -> Option<Id<UserMarker>> {
        self.access.owner()
    }
}

impl StaffGate for GuildStaffGate {
    fn check<'a>(&'a self, discord_user_id: &'a str) -> GateFuture<'a, StaffCheck> {
        Box::pin(async move {
            let Some(user_id) = discord_user_id
                .parse::<u64>()
                .ok()
                .and_then(Id::<UserMarker>::new_checked)
            else {
                return StaffCheck::NotStaff;
            };
            match self.members.member(user_id).await {
                Ok(Some(invoker)) if self.policy.is_staff(&invoker, self.members.owner_id()) => {
                    StaffCheck::Staff
                }
                Ok(_) => StaffCheck::NotStaff,
                Err(()) => StaffCheck::Unavailable,
            }
        })
    }
}
