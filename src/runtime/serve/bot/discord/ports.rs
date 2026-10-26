//! The store and access policy behind the bot's card ports.

use std::sync::Arc;

use crate::{
    app::{guild::GuildAccess, ids::id_text},
    bot::cards::Authority,
    discord::roster::LiveRoster,
};
use kanade_domain::{
    notify::{CardIndex, LookupError},
    proposals::Approver,
};
use kanade_store::SqliteStore;

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
    async fn runs_for_message(&self, message_id: &str) -> Result<Vec<String>, LookupError> {
        self.0.runs_for_message(message_id).await
    }
}
