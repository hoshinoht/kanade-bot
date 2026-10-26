//! One roster pass against the store: prune roles the guild no longer has,
//! record its owner and Administrator roles, page every member and apply the
//! difference, then refresh the in-memory snapshot. The gateway's roster
//! task runs it per fresh generation; a server without the bot runs it over
//! REST on a timer.

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::sync::watch;
use twilight_model::id::{Id, marker::UserMarker};

use super::live::LiveRoster;
use super::{ReconcileReport, diff, fetch_members};
use crate::app::ids::id_text;
use crate::app::roster::{AdminRoles, RosterUpdate};
use crate::discord::guild::GuildScope;
use crate::discord::guild_cache::GuildCache;
use crate::discord::transport::DiscordTransport;
use kanade_domain::members::{GatewayMember, MemberProfile};
use kanade_domain::scheduler::StoreError;

/// Where roster changes are persisted and sessions re-checked. Every method
/// returns the admin sessions it ended.
pub trait RosterSink: Send + Sync {
    fn members(&self) -> impl Future<Output = Result<Vec<MemberProfile>, StoreError>> + Send;

    fn update(&self, update: &RosterUpdate)
    -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Rewrite a row's gateway fields (roles pruned of deleted ones).
    fn prune(&self, member: GatewayMember) -> impl Future<Output = Result<u64, StoreError>> + Send;

    fn guild_available(
        &self,
        owner_id: Id<UserMarker>,
        admin_roles: &AdminRoles,
    ) -> impl Future<Output = Result<u64, StoreError>> + Send;
}

/// Rows naming a role the guild no longer has, rewritten without it. The
/// bossing flag and Administrator can only be lost here, never gained.
pub fn prune_roles(
    profiles: &[MemberProfile],
    known: &BTreeSet<String>,
    bossing_role: &str,
    admin: &AdminRoles,
) -> Vec<GatewayMember> {
    profiles
        .iter()
        .filter(|row| row.roles.iter().any(|role| !known.contains(role)))
        .map(|row| {
            let roles: Vec<String> = row
                .roles
                .iter()
                .filter(|role| known.contains(*role))
                .cloned()
                .collect();
            GatewayMember {
                user_id: row.member.user_id.clone(),
                display_name: row.member.display_name.clone(),
                nickname: row.member.nickname.clone(),
                has_role: row.member.has_role && known.contains(bossing_role),
                is_bot: row.member.is_bot,
                is_guild_admin: row.is_guild_admin && admin.grants(&roles),
                roles,
            }
        })
        .collect()
}

/// The store side of the roster and the guild's member list, with the last
/// Administrator roles applied.
pub struct RosterSync<K, T> {
    sink: K,
    transport: Arc<T>,
    cache: Arc<GuildCache>,
    scope: GuildScope,
    live: Arc<LiveRoster>,
    stop: watch::Receiver<bool>,
    admin_roles: AdminRoles,
}

impl<K: RosterSink, T: DiscordTransport> RosterSync<K, T> {
    pub fn new(
        sink: K,
        transport: Arc<T>,
        cache: Arc<GuildCache>,
        scope: GuildScope,
        live: Arc<LiveRoster>,
        stop: watch::Receiver<bool>,
    ) -> Self {
        Self {
            sink,
            transport,
            cache,
            scope,
            live,
            stop,
            admin_roles: AdminRoles::default(),
        }
    }

    /// Whether the stop signal has fired.
    pub fn stopped(&self) -> bool {
        *self.stop.borrow()
    }

    /// The snapshot this pass refreshes.
    pub fn live(&self) -> &LiveRoster {
        &self.live
    }

    /// Replace the snapshot from the store; `false` when the read failed.
    pub async fn refresh(&self) -> bool {
        match self.sink.members().await {
            Ok(rows) => {
                self.live.replace(rows);
                true
            }
            Err(error) => {
                failed("members", &error);
                false
            }
        }
    }

    /// Persist one member change; a failed write is logged.
    pub async fn update(&self, update: &RosterUpdate) {
        if let Err(error) = self.sink.update(update).await {
            failed("update", &error);
        }
    }

    /// Prune roles the guild no longer has, then record its owner and
    /// Administrator roles. `false` when a step failed.
    pub async fn apply_guild_available(
        &mut self,
        owner_id: Id<UserMarker>,
        admin_roles: &AdminRoles,
    ) -> bool {
        self.admin_roles = admin_roles.clone();
        let pruned = self.prune().await;
        let recorded = match self.sink.guild_available(owner_id, &self.admin_roles).await {
            Ok(_) => true,
            Err(error) => {
                failed("guild_available", &error);
                false
            }
        };
        pruned && recorded
    }

    /// One full pass: [`Self::apply_guild_available`], then page every
    /// member and apply the difference, sessions included, then refresh the
    /// snapshot. `false` when a step failed; a failed page concludes no
    /// departures.
    pub async fn reconcile_now(
        &mut self,
        owner_id: Id<UserMarker>,
        admin_roles: &AdminRoles,
    ) -> bool {
        if !self.apply_guild_available(owner_id, admin_roles).await {
            self.refresh().await;
            return false;
        }
        if !self.reconcile().await {
            self.refresh().await;
            return false;
        }
        self.refresh().await
    }

    async fn prune(&self) -> bool {
        let Some(known) = self.cache.role_ids() else {
            return false;
        };
        let known: BTreeSet<String> = known.into_iter().map(id_text).collect();
        let rows = match self.sink.members().await {
            Ok(rows) => rows,
            Err(error) => {
                failed("members", &error);
                return false;
            }
        };
        let bossing = id_text(self.scope.bossing_role_id);
        let mut complete = true;
        for member in prune_roles(&rows, &known, &bossing, &self.admin_roles) {
            if let Err(error) = self.sink.prune(member).await {
                failed("prune", &error);
                complete = false;
            }
        }
        complete
    }

    async fn reconcile(&self) -> bool {
        let stop = self.stop.clone();
        let fetched =
            match fetch_members(&*self.transport, self.scope.guild_id, || *stop.borrow()).await {
                Ok(fetched) => fetched,
                Err(error) => {
                    kanade_log::log!(
                        warn,
                        "roster_reconcile_failed",
                        read = error.read,
                        outcome = error.outcome,
                    );
                    return false;
                }
            };
        self.cache.replace_member_avatars(&fetched);
        // The bot's own roles decide its channel permissions.
        if let Some(me) = self
            .cache
            .self_id()
            .and_then(|me| fetched.iter().find(|member| member.user.id == me))
        {
            self.cache.member_roles(me.user.id, &me.roles);
        }
        let stored = match self.sink.members().await {
            Ok(rows) => rows,
            Err(error) => {
                failed("members", &error);
                return false;
            }
        };
        let mut report = ReconcileReport {
            members: fetched.len(),
            ..ReconcileReport::default()
        };
        let mut complete = true;
        for update in diff(
            &fetched,
            &stored,
            self.scope.bossing_role_id,
            &self.admin_roles,
        ) {
            match self.sink.update(&update).await {
                Ok(ended) => {
                    report.sessions_ended += ended;
                    match update {
                        RosterUpdate::Seen { .. } => report.seen += 1,
                        RosterUpdate::Left { .. } => report.left += 1,
                    }
                }
                Err(error) => {
                    failed("update", &error);
                    complete = false;
                }
            }
        }
        if !complete {
            return false;
        }
        kanade_log::log!(
            info,
            "roster_reconciled",
            members = report.members,
            seen = report.seen,
            left = report.left,
            sessions_ended = report.sessions_ended,
        );
        true
    }
}

/// Store error text can carry paths; only its kind is logged.
fn failed(step: &'static str, error: &StoreError) {
    let kind = match error {
        StoreError::Conflict { .. } => "conflict",
        StoreError::Constraint(_) => "constraint",
        _ => "backend",
    };
    kanade_log::log!(warn, "roster_write_failed", step, kind);
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use tokio::sync::watch;
    use twilight_model::id::{Id, marker::RoleMarker};

    use super::{RosterSink, RosterSync};
    use crate::{
        app::roster::RosterUpdate,
        discord::{
            guild::GuildScope, guild_cache::GuildCache, roster::LiveRoster, transport::FakeDiscord,
        },
    };
    use kanade_domain::{
        members::{GatewayMember, Member, MemberProfile},
        scheduler::StoreError,
    };

    struct FailingUpdate {
        profile: MemberProfile,
        fail_once: AtomicBool,
    }

    impl RosterSink for FailingUpdate {
        async fn members(&self) -> Result<Vec<MemberProfile>, StoreError> {
            Ok(vec![self.profile.clone()])
        }

        async fn update(&self, _: &RosterUpdate) -> Result<u64, StoreError> {
            if self.fail_once.swap(false, Ordering::SeqCst) {
                Err(StoreError::Backend("test failure".into()))
            } else {
                Ok(0)
            }
        }

        async fn prune(&self, _: GatewayMember) -> Result<u64, StoreError> {
            Ok(0)
        }

        async fn guild_available(
            &self,
            _: Id<twilight_model::id::marker::UserMarker>,
            _: &crate::app::roster::AdminRoles,
        ) -> Result<u64, StoreError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn failed_member_update_does_not_report_reconciliation_complete() {
        let cache = Arc::new(GuildCache::new(Id::new(1)));
        let (_, stop) = watch::channel(false);
        let sink = FailingUpdate {
            profile: MemberProfile {
                member: Member {
                    user_id: "1001".into(),
                    has_role: true,
                    ..Member::default()
                },
                roles: vec!["2".into()],
                ..MemberProfile::default()
            },
            fail_once: AtomicBool::new(true),
        };
        let sync = RosterSync::new(
            sink,
            Arc::new(FakeDiscord::new()),
            Arc::clone(&cache),
            GuildScope {
                guild_id: Id::new(1),
                bossing_role_id: Id::<RoleMarker>::new(2),
            },
            Arc::new(LiveRoster::new(cache)),
            stop,
        );

        assert!(
            !sync.reconcile().await,
            "a failed write must keep readiness closed"
        );
        assert!(sync.reconcile().await, "the repeated diff succeeds");
    }
}
