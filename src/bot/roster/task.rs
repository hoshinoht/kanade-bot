//! The one sequential roster task. Guild availability, member updates and
//! reconciliation are applied in arrival order, so an older write can never
//! land over a newer one.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep_until};
use twilight_model::id::{Id, marker::UserMarker};

use crate::app::roster::{AdminRoles, RosterUpdate};
use crate::bot::gateway::ConnectionStatus;
use crate::discord::guild::GuildScope;
use crate::discord::guild_cache::GuildCache;
use crate::discord::roster::{LiveRoster, RosterSink, RosterSync};
use crate::discord::transport::DiscordTransport;

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

pub struct RosterTask<K, T> {
    sync: RosterSync<K, T>,
    connection: ConnectionStatus,
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
            sync: RosterSync::new(sink, transport, cache, scope, live, stop),
            connection,
        }
    }

    /// Apply jobs until every sender is dropped. The snapshot is refreshed
    /// once the queue is empty, so a burst costs one read. A failed generation
    /// has one delayed exponential retry slot, capped at one minute.
    pub async fn run(mut self, mut jobs: mpsc::UnboundedReceiver<RosterJob>) {
        self.sync.refresh().await;
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
                self.sync.refresh().await;
            }
        }
    }

    async fn handle(&mut self, job: RosterJob) -> Option<RosterJob> {
        match job {
            RosterJob::Update(update) => {
                self.sync.update(&update).await;
                None
            }
            RosterJob::GuildAvailable {
                owner_id,
                admin_roles,
            } => {
                let _ = self
                    .sync
                    .apply_guild_available(owner_id, &admin_roles)
                    .await;
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
                if self.sync.stopped() || !self.connection.is_current_fresh_generation(generation) {
                    return None;
                }
                if self.sync.reconcile_now(owner_id, &admin_roles).await
                    && !self.sync.stopped()
                    && self.connection.is_current_fresh_generation(generation)
                {
                    self.connection.roster_reconciled(generation);
                    self.sync.live().mark_reconciled();
                    None
                } else if !self.sync.stopped()
                    && self.connection.is_current_fresh_generation(generation)
                {
                    Some(retry)
                } else {
                    None
                }
            }
        }
    }
}

fn reconcile_retry_delay(attempts: u32) -> Duration {
    let exponent = attempts.saturating_sub(1).min(6);
    Duration::from_secs((1_u64 << exponent).min(RECONCILE_RETRY_MAX_SECONDS))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::reconcile_retry_delay;

    #[test]
    fn reconciliation_retry_delay_grows_then_caps() {
        assert_eq!(reconcile_retry_delay(1), Duration::from_secs(1));
        assert_eq!(reconcile_retry_delay(2), Duration::from_secs(2));
        assert_eq!(reconcile_retry_delay(7), Duration::from_secs(60));
        assert_eq!(reconcile_retry_delay(100), Duration::from_secs(60));
    }
}
