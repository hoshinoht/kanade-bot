//! The store, sessions and access policy behind the bot's ports.

use std::sync::Arc;

use twilight_model::id::{Id, marker::MessageMarker, marker::UserMarker};

use crate::{
    api::{
        auth::{
            AdminAuth,
            member::MemberAuth,
            roster::{on_guild_available, on_roster_update},
        },
        avatars::AvatarCache,
        state::GuildAccess,
    },
    bot::{
        cards::Authority,
        events::{AdminRoles, CardIndex, LookupError, RosterUpdate},
        ids::id_text,
        roster::{LiveRoster, RosterSink},
    },
    domain::{
        members::{GatewayMember, MemberProfile, MemberStore},
        proposals::Approver,
        scheduler::StoreError,
    },
    infrastructure::store::SqliteStore,
};

/// Roster writes on the store, each followed by both realms' session re-checks.
pub struct StoreRoster {
    pub store: Arc<SqliteStore>,
    pub auth: Arc<AdminAuth>,
    /// The public portal's realm; `None` without a public listener.
    pub member: Option<Arc<MemberAuth>>,
    pub access: Arc<GuildAccess>,
    pub avatars: Option<Arc<AvatarCache>>,
}

impl RosterSink for StoreRoster {
    async fn members(&self) -> Result<Vec<MemberProfile>, StoreError> {
        self.store.list_members().await
    }

    async fn update(&self, update: &RosterUpdate) -> Result<u64, StoreError> {
        on_roster_update(
            &self.auth,
            self.member.as_deref(),
            &*self.store,
            self.avatars.as_deref(),
            update,
        )
        .await
    }

    async fn prune(&self, member: GatewayMember) -> Result<u64, StoreError> {
        let user_id = member.user_id.clone();
        let eligible = member.has_role && !member.is_bot;
        self.store.apply_gateway(member).await?;
        let public = match &self.member {
            Some(realm) => realm.member_changed(&user_id, eligible).await,
            None => 0,
        };
        Ok(self.auth.member_changed(&user_id).await + public)
    }

    async fn guild_available(
        &self,
        owner_id: Id<UserMarker>,
        admin_roles: &AdminRoles,
    ) -> Result<u64, StoreError> {
        on_guild_available(
            &self.auth,
            &self.access,
            &*self.store,
            owner_id,
            admin_roles,
        )
        .await
    }
}

/// A ✅ on a card is the member's own, never `via_portal`; staff (the staff
/// rule over the stored gateway view, or the owner) approve anything.
pub struct StaffAuthority {
    pub roster: Arc<LiveRoster>,
    pub access: Arc<GuildAccess>,
}

impl Authority for StaffAuthority {
    fn approver(&self, user_id: &str) -> Approver {
        let profile = self.roster.profile(user_id);
        let owner = self.access.owner();
        let is_owner = owner.is_some_and(|owner| id_text(owner) == user_id);
        let is_staff = profile
            .as_ref()
            .and_then(GuildAccess::invoker)
            .is_some_and(|invoker| self.access.policy.is_staff(&invoker, owner));
        Approver {
            user_id: user_id.to_owned(),
            has_role: profile
                .as_ref()
                .is_some_and(|profile| profile.member.has_role && !profile.member.is_bot),
            is_admin: is_staff || is_owner,
            via_portal: false,
        }
    }
}

/// The card index over the shared store.
pub struct StoreIndex(pub Arc<SqliteStore>);

impl CardIndex for StoreIndex {
    async fn runs_for_message(
        &self,
        message: Id<MessageMarker>,
    ) -> Result<Vec<String>, LookupError> {
        self.0.runs_for_message(message).await
    }
}
