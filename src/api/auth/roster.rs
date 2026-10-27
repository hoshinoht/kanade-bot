//! Both realms' session hooks for the bot's roster events
//! ([`crate::app::roster`] persists the member rows first).

use std::sync::Arc;

use super::{AdminAuth, member::MemberAuth};
use crate::{
    app::roster::{HookFuture, RosterHooks},
    discord::avatars::AvatarCache,
};

/// Ends sessions that no longer pass in the admin realm and the public
/// portal's, and purges a departed member's cached portrait.
pub struct SessionHooks {
    pub auth: Arc<AdminAuth>,
    /// The public portal's realm; `None` without a public listener.
    pub member: Option<Arc<MemberAuth>>,
    pub avatars: Option<Arc<AvatarCache>>,
}

impl RosterHooks for SessionHooks {
    fn changed<'a>(&'a self, user_id: &'a str, eligible: bool) -> HookFuture<'a> {
        Box::pin(async move {
            let public = match &self.member {
                Some(member) => member.member_changed(user_id, eligible).await,
                None => 0,
            };
            self.auth.member_changed(user_id).await + public
        })
    }

    fn left<'a>(&'a self, user_id: &'a str) -> HookFuture<'a> {
        Box::pin(async move {
            if let Some(avatars) = &self.avatars {
                avatars.purge(user_id);
            }
            let public = match &self.member {
                Some(member) => member.member_left(user_id).await,
                None => 0,
            };
            self.auth.member_left(user_id).await + public
        })
    }

    fn staff_changed<'a>(&'a self, user_id: &'a str) -> HookFuture<'a> {
        Box::pin(self.auth.member_changed(user_id))
    }
}
