//! The one sequential roster task. Guild availability, member updates and
//! reconciliation are applied in arrival order, so an older write can never
//! land over a newer one.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep_until};
use twilight_model::id::{Id, marker::UserMarker};

use super::live::LiveRoster;
use super::reconcile::{ReconcileReport, diff, fetch_members};
use crate::bot::events::{AdminRoles, GuildScope, RosterUpdate};
use crate::bot::gateway::ConnectionStatus;
use crate::bot::guild_cache::GuildCache;
use crate::bot::ids::id_text;
use crate::bot::transport::DiscordTransport;
use crate::domain::members::{GatewayMember, MemberProfile};
use crate::domain::scheduler::StoreError;
use crate::runtime::logging;

const RECONCILE_RETRY_MAX_SECONDS: u64 = 60;

/// Work for the roster task, in gateway order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RosterJob {
    GuildAvailable {
        owner_id: Id<UserMarker>,
        admin_roles: AdminRoles,
    },
    Update(RosterUpdate),
    /// Page the guild's members and apply the difference.
    Reconcile {
        generation: u64,
        owner_id: Id<UserMarker>,
        admin_roles: AdminRoles,
    },
}

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

pub struct RosterTask<K, T> {
    sink: K,
    transport: Arc<T>,
    cache: Arc<GuildCache>,
    scope: GuildScope,
    live: Arc<LiveRoster>,
    connection: ConnectionStatus,
    stop: watch::Receiver<bool>,
    admin_roles: AdminRoles,
}

impl<K: RosterSink, T: DiscordTransport> RosterTask<K, T> {
    pub fn new(
        sink: K,
        transport: Arc<T>,
        cache: Arc<GuildCache>,
        scope: GuildScope,
        live: Arc<LiveRoster>,
        connection: ConnectionStatus,
        stop: watch::Receiver<bool>,
    ) -> Self {
        Self {
            sink,
            transport,
            cache,
            scope,
            live,
            connection,
            stop,
            admin_roles: AdminRoles::default(),
        }
    }

    /// Apply jobs until every sender is dropped. The snapshot is refreshed
    /// once the queue is empty, so a burst costs one read. A failed generation
    /// has one delayed exponential retry slot, capped at one minute.
    pub async fn run(mut self, mut jobs: mpsc::UnboundedReceiver<RosterJob>) {
        self.refresh().await;
        let mut retry: Option<(RosterJob, Instant, u32)> = None;
        loop {
            let next = match retry
                .as_ref()
                .map(|(job, at, attempts)| (job.clone(), *at, *attempts))
            {
                Some((retry_job, retry_at, attempts)) => {
                    tokio::select! {
                        biased;
                        _ = sleep_until(retry_at) => {
                            retry = None;
                            Some((retry_job, Some(attempts)))
                        },
                        job = jobs.recv() => job.map(|job| (job, None)),
                    }
                }
                None => jobs.recv().await.map(|job| (job, None)),
            };
            let Some((job, previous_attempts)) = next else {
                break;
            };
            if let (
                Some((
                    RosterJob::Reconcile {
                        generation,
                        owner_id: retry_owner,
                        admin_roles: retry_roles,
                    },
                    _,
                    _,
                )),
                RosterJob::GuildAvailable {
                    owner_id,
                    admin_roles,
                },
            ) = (&mut retry, &job)
                && self.connection.is_current_fresh_generation(*generation)
            {
                *retry_owner = *owner_id;
                *retry_roles = admin_roles.clone();
            }
            let is_reconcile = matches!(&job, RosterJob::Reconcile { .. });
            if is_reconcile {
                retry = None;
            }
            if let Some(job) = self.handle(job).await {
                let attempts = previous_attempts.unwrap_or_default().saturating_add(1);
                retry = Some((
                    job,
                    Instant::now() + reconcile_retry_delay(attempts),
                    attempts,
                ));
            }
            if jobs.is_empty() && !is_reconcile {
                self.refresh().await;
            }
        }
    }

    async fn refresh(&self) -> bool {
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

    async fn handle(&mut self, job: RosterJob) -> Option<RosterJob> {
        match job {
            RosterJob::Update(update) => {
                if let Err(error) = self.sink.update(&update).await {
                    failed("update", &error);
                }
                None
            }
            RosterJob::GuildAvailable {
                owner_id,
                admin_roles,
            } => {
                let _ = self.apply_guild_available(owner_id, &admin_roles).await;
                None
            }
            RosterJob::Reconcile {
                generation,
                owner_id,
                admin_roles,
            } => {
                let retry = RosterJob::Reconcile {
                    generation,
                    owner_id,
                    admin_roles: admin_roles.clone(),
                };
                if *self.stop.borrow() || !self.connection.is_current_fresh_generation(generation) {
                    return None;
                }
                if self.reconcile_generation(owner_id, &admin_roles).await
                    && !*self.stop.borrow()
                    && self.connection.is_current_fresh_generation(generation)
                {
                    self.connection.roster_reconciled(generation);
                    self.live.mark_reconciled();
                    None
                } else if !*self.stop.borrow()
                    && self.connection.is_current_fresh_generation(generation)
                {
                    Some(retry)
                } else {
                    None
                }
            }
        }
    }

    async fn apply_guild_available(
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

    async fn reconcile_generation(
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
                    logging::event(
                        "WARN",
                        "roster_reconcile_failed",
                        json!({"read": error.read, "outcome": error.outcome}),
                    );
                    return false;
                }
            };
        self.cache.replace_member_avatars(&fetched);
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
        logging::event(
            "INFO",
            "roster_reconciled",
            json!({
                "members": report.members,
                "seen": report.seen,
                "left": report.left,
                "sessions_ended": report.sessions_ended,
            }),
        );
        true
    }
}

fn reconcile_retry_delay(attempts: u32) -> Duration {
    let exponent = attempts.saturating_sub(1).min(6);
    Duration::from_secs((1_u64 << exponent).min(RECONCILE_RETRY_MAX_SECONDS))
}

/// Store error text can carry paths; only its kind is logged.
fn failed(step: &'static str, error: &StoreError) {
    let kind = match error {
        StoreError::Conflict { .. } => "conflict",
        StoreError::Constraint(_) => "constraint",
        _ => "backend",
    };
    logging::event(
        "WARN",
        "roster_write_failed",
        json!({"step": step, "kind": kind}),
    );
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use tokio::sync::watch;
    use twilight_model::id::{Id, marker::RoleMarker};

    use super::{RosterSink, RosterTask, reconcile_retry_delay};
    use crate::bot::{
        events::{GuildScope, RosterUpdate},
        gateway::ConnectionStatus,
        guild_cache::GuildCache,
        roster::LiveRoster,
        transport::FakeDiscord,
    };
    use crate::domain::{
        members::{GatewayMember, Member, MemberProfile},
        scheduler::StoreError,
    };

    #[test]
    fn reconciliation_retry_delay_grows_then_caps() {
        assert_eq!(reconcile_retry_delay(1), Duration::from_secs(1));
        assert_eq!(reconcile_retry_delay(2), Duration::from_secs(2));
        assert_eq!(reconcile_retry_delay(7), Duration::from_secs(60));
        assert_eq!(reconcile_retry_delay(100), Duration::from_secs(60));
    }

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
            _: &crate::bot::events::AdminRoles,
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
        let task = RosterTask::new(
            sink,
            Arc::new(FakeDiscord::new()),
            Arc::clone(&cache),
            GuildScope {
                guild_id: Id::new(1),
                bossing_role_id: Id::<RoleMarker>::new(2),
            },
            Arc::new(LiveRoster::new(cache)),
            ConnectionStatus::new(),
            stop,
        );

        assert!(
            !task.reconcile().await,
            "a failed write must keep readiness closed"
        );
        assert!(task.reconcile().await, "the repeated diff succeeds");
    }
}
