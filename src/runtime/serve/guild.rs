//! The configured guild as both halves of serve read it over Discord's REST
//! API: its scope, the extractor's watch list, the difficulty marks listed
//! at startup and the roster's store sink.

use std::sync::Arc;
use std::time::Duration;

use twilight_model::id::{Id, marker::UserMarker};

use crate::{
    app::{
        guild::GuildAccess,
        ids::parse_id,
        roster::{AdminRoles, RosterHooks, RosterUpdate, on_guild_available, on_roster_update},
    },
    cards::{
        DifficultyMarks,
        emojis::{self, PILLS},
    },
    discord::{
        guild::GuildScope,
        guild_cache::WatchList,
        roster::RosterSink,
        transport::{DiscordTransport, Outcome},
    },
    runtime::config::ServeConfig,
};
use kanade_domain::{
    members::{GatewayMember, MemberProfile, MemberStore},
    scheduler::StoreError,
    settings::RuntimeSettings,
};
use kanade_store::SqliteStore;

/// How long serve waits at startup for the application's emoji list.
const MARKS_TIMEOUT: Duration = Duration::from_secs(15);

pub fn scope(config: &ServeConfig) -> GuildScope {
    // Snowflakes are validated non-zero by the config parser.
    GuildScope {
        guild_id: Id::new(config.guild.guild_id),
        bossing_role_id: Id::new(config.guild.bossing_role_id),
    }
}

/// The extractor's watch list from settings; unparsable ids are skipped.
pub fn watch_list(settings: &RuntimeSettings) -> WatchList {
    let ids = |list: &[String]| list.iter().filter_map(|id| parse_id(id)).collect();
    WatchList {
        channel_ids: ids(&settings.watching.channel_ids),
        category_ids: ids(&settings.watching.category_ids),
    }
}

/// The difficulty pills among the application's emojis, listed once at
/// startup. A missing pill, or any failure, leaves that label written out.
pub async fn difficulty_marks<T: DiscordTransport>(transport: &T) -> DifficultyMarks {
    let listed = tokio::time::timeout(MARKS_TIMEOUT, emojis::difficulty_marks(transport)).await;
    let reason = match listed {
        Ok(Outcome::Delivered(marks)) => {
            let missing: Vec<&str> = PILLS
                .iter()
                .filter(|(letter, _)| marks.get(letter).is_none())
                .map(|(_, name)| *name)
                .collect();
            let level = if missing.is_empty() { "INFO" } else { "WARN" };
            kanade_log::log!(level, "difficulty_marks", missing);
            return marks;
        }
        Ok(failed) => failed.failure_label().unwrap_or_default(),
        Err(_) => "timeout".to_owned(),
    };
    kanade_log::log!(warn, "difficulty_marks_unavailable", reason);
    DifficultyMarks::default()
}

/// Roster writes on the store, each followed by the web side's session
/// re-checks.
pub struct StoreRoster {
    pub store: Arc<SqliteStore>,
    pub hooks: Arc<dyn RosterHooks>,
    pub access: Arc<GuildAccess>,
}

impl RosterSink for StoreRoster {
    async fn members(&self) -> Result<Vec<MemberProfile>, StoreError> {
        self.store.list_members().await
    }

    async fn update(&self, update: &RosterUpdate) -> Result<u64, StoreError> {
        on_roster_update(&*self.hooks, &*self.store, update).await
    }

    async fn prune(&self, member: GatewayMember) -> Result<u64, StoreError> {
        let user_id = member.user_id.clone();
        let eligible = member.has_role && !member.is_bot;
        self.store.apply_gateway(member).await?;
        Ok(self.hooks.changed(&user_id, eligible).await)
    }

    async fn guild_available(
        &self,
        owner_id: Id<UserMarker>,
        admin_roles: &AdminRoles,
    ) -> Result<u64, StoreError> {
        on_guild_available(
            &*self.hooks,
            &self.access,
            &*self.store,
            owner_id,
            admin_roles,
        )
        .await
    }
}
