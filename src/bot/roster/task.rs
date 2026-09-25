//! The one sequential roster task. Guild availability, member updates and
//! reconciliation are applied in arrival order, so an older write can never
//! land over a newer one.

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::{mpsc, watch};
use twilight_model::id::{Id, marker::UserMarker};

use super::live::LiveRoster;
use super::reconcile::{ReconcileReport, diff, fetch_members};
use crate::bot::events::{AdminRoles, GuildScope, RosterUpdate};
use crate::bot::guild_cache::GuildCache;
use crate::bot::ids::id_text;
use crate::bot::transport::DiscordTransport;
use crate::domain::members::{GatewayMember, MemberProfile};
use crate::domain::scheduler::StoreError;
use crate::runtime::logging;

/// Work for the roster task, in gateway order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RosterJob {
    GuildAvailable {
        owner_id: Id<UserMarker>,
        admin_roles: AdminRoles,
    },
    Update(RosterUpdate),
    /// Page the guild's members and apply the difference.
    Reconcile,
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

    /// Apply jobs until every sender is dropped. The snapshot is refreshed
    /// once the queue is empty, so a burst costs one read.
    pub async fn run(mut self, mut jobs: mpsc::UnboundedReceiver<RosterJob>) {
        self.refresh().await;
        while let Some(job) = jobs.recv().await {
            self.handle(job).await;
            if jobs.is_empty() {
                self.refresh().await;
            }
        }
    }

    async fn refresh(&self) {
        match self.sink.members().await {
            Ok(rows) => self.live.replace(rows),
            Err(error) => failed("members", &error),
        }
    }

    async fn handle(&mut self, job: RosterJob) {
        match job {
            RosterJob::Update(update) => {
                if let Err(error) = self.sink.update(&update).await {
                    failed("update", &error);
                }
            }
            RosterJob::GuildAvailable {
                owner_id,
                admin_roles,
            } => {
                self.admin_roles = admin_roles;
                self.prune().await;
                if let Err(error) = self.sink.guild_available(owner_id, &self.admin_roles).await {
                    failed("guild_available", &error);
                }
            }
            RosterJob::Reconcile => {
                if !*self.stop.borrow() {
                    self.reconcile().await;
                }
            }
        }
    }

    async fn prune(&self) {
        let Some(known) = self.cache.role_ids() else {
            return;
        };
        let known: BTreeSet<String> = known.into_iter().map(id_text).collect();
        let rows = match self.sink.members().await {
            Ok(rows) => rows,
            Err(error) => return failed("members", &error),
        };
        let bossing = id_text(self.scope.bossing_role_id);
        for member in prune_roles(&rows, &known, &bossing, &self.admin_roles) {
            if let Err(error) = self.sink.prune(member).await {
                failed("prune", &error);
            }
        }
    }

    async fn reconcile(&self) {
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
                    return;
                }
            };
        let stored = match self.sink.members().await {
            Ok(rows) => rows,
            Err(error) => return failed("members", &error),
        };
        let mut report = ReconcileReport {
            members: fetched.len(),
            ..ReconcileReport::default()
        };
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
                Err(error) => failed("update", &error),
            }
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
    }
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
