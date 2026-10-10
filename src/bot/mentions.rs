//! The one place Discord allowed-mentions are built.
//!
//! Only explicitly listed users may be notified: `parse` is always empty (so
//! `@everyone`, `@here`, role and unlisted user mentions in content ping
//! nobody), roles are never listed and `replied_user` is off. An empty list
//! mentions nobody, matching v4's `AllowedMentions.none()` for quiet posts.

use twilight_model::channel::message::AllowedMentions;
use twilight_model::id::{Id, marker::UserMarker};

use super::ids::parse_id;
use crate::domain::notify::{self, NotificationIntent};

/// Allow exactly these users; unparsable ids are dropped, never widened.
/// The list is sorted and deduplicated to match the journal's canonical form.
pub fn allow_users<S: AsRef<str>>(users: &[S]) -> AllowedMentions {
    let mut ids: Vec<Id<UserMarker>> = users
        .iter()
        .filter_map(|user| parse_id(user.as_ref()))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    AllowedMentions {
        parse: Vec::new(),
        replied_user: false,
        roles: Vec::new(),
        users: ids,
    }
}

/// Mention nobody.
pub fn none() -> AllowedMentions {
    allow_users::<&str>(&[])
}

/// The allow-list for an intent: its quiet-gated user list and nothing else.
pub fn for_intent(intent: &NotificationIntent) -> AllowedMentions {
    allow_users(&intent.mentions)
}

/// The allow-list for the domain policy value. `replied_user` stays off even
/// for [`notify::AllowedMentions::Users`]: the adapter never sends replies
/// that should notify their author.
pub fn for_policy(policy: &notify::AllowedMentions) -> AllowedMentions {
    allow_users(policy.users())
}
