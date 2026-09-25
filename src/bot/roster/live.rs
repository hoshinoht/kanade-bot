//! The stored member rows, held in memory for synchronous readers.

use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use crate::bot::guild_cache::GuildCache;
use crate::bot::ids::parse_id;
use crate::domain::members::{Directory, Member, MemberProfile};

/// A snapshot of the `members` table plus the guild cache's watch list.
/// Refreshed by the roster task after its writes and by the tick before
/// each run (portal edits such as ping levels land there).
#[derive(Debug)]
pub struct LiveRoster {
    cache: Arc<GuildCache>,
    rows: RwLock<Arc<BTreeMap<String, MemberProfile>>>,
}

impl LiveRoster {
    pub fn new(cache: Arc<GuildCache>) -> Self {
        Self {
            cache,
            rows: RwLock::default(),
        }
    }

    pub fn replace(&self, profiles: Vec<MemberProfile>) {
        let rows = profiles
            .into_iter()
            .map(|profile| (profile.member.user_id.clone(), profile))
            .collect();
        *self.rows.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(rows);
    }

    pub fn profile(&self, user_id: &str) -> Option<MemberProfile> {
        self.rows
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(user_id)
            .cloned()
    }
}

impl Directory for LiveRoster {
    fn member(&self, user_id: &str) -> Option<Member> {
        self.profile(user_id).map(|profile| profile.member)
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        parse_id(channel_id).is_some_and(|id| self.cache.is_watched(id))
    }
}
