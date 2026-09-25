//! Draining the notice outbox: each pending notice is planned (channel,
//! mentions) and rendered now, claimed by its durable `(source, ordinal)`
//! key, sent through the executor, then marked drained.
//!
//! Crash windows: a crash before the claim leaves the notice pending and the
//! next tick sends it once; a crash after the claim leaves an `intent` that
//! restart recovery makes indeterminate, and the next claim of the same key
//! is `Held`, so it is drained without a second send. Only a send the
//! transport proved undelivered (`NotSent`, rate limited) stays pending and
//! is claimed afresh.

use chrono::{DateTime, Utc};

use super::alerts::AlertSink;
use super::executor::{SendOutcome, SendReport};
use super::render::render;
use super::tick::{Delivery, DeliveryError, settle};
use crate::bot::transport::DiscordTransport;
use crate::domain::drafts::ProposalStore;
use crate::domain::history::Checkpoints;
use crate::domain::notify::{DeliveryJournal, Lease, NoticeOutbox, plan_notice};
use crate::domain::scheduler::{IdSource, ScheduleStore, Scope};

/// One drained (or retried) notice.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoticeSend {
    pub source: String,
    pub ordinal: i64,
    pub send: SendReport,
    /// Marked drained: sent, held by an earlier claim, uncertain or rejected.
    pub drained: bool,
}

/// What one drain did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NoticeReport {
    pub sends: Vec<NoticeSend>,
    /// Pending with neither its home channel nor the post channel reachable;
    /// retried next tick, as queued reminders are.
    pub unroutable: usize,
    /// Not attempted because the per-tick cap was reached.
    pub deferred: usize,
}

/// Released sends never reached Discord and a failed journal write is
/// retried by claim; everything else is final for the outbox.
fn drains(outcome: &SendOutcome) -> bool {
    !matches!(outcome, SendOutcome::Released(_) | SendOutcome::Failed(_))
}

impl<S, I, T, A> Delivery<'_, S, I, T, A>
where
    S: ScheduleStore + DeliveryJournal + NoticeOutbox + Checkpoints + ProposalStore + Sync,
    I: IdSource,
    T: DiscordTransport,
    A: AlertSink,
{
    /// The notice step alone, under its own lease.
    ///
    /// # Errors
    /// Lease loss or a journal/store failure.
    pub async fn drain_notices(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<NoticeReport, DeliveryError> {
        self.leased(now, async move |this: &mut Self, lease: &Lease| {
            this.notices_in(lease, now).await
        })
        .await
    }

    /// Send pending notices in write order, at most `max_sends_per_tick`
    /// claims per call.
    pub(super) async fn notices_in(
        &self,
        lease: &Lease,
        now: DateTime<Utc>,
    ) -> Result<NoticeReport, DeliveryError> {
        let pending = self.store.pending_notices().await?;
        let mut report = NoticeReport::default();
        if pending.is_empty() {
            return Ok(report);
        }
        let schedule = self.store.load(&Scope::All).await?;
        let executor = self.executor(lease);
        let limit = self.config.max_sends_per_tick;
        let mut claimed = 0;
        for row in pending {
            let Some(intent) = plan_notice(
                &row.notice,
                self.members,
                self.channels,
                self.config.settings(),
            ) else {
                report.unroutable += 1;
                continue;
            };
            if claimed >= limit {
                report.deferred += 1;
                continue;
            }
            let message = render(
                &intent,
                &schedule,
                self.config.policy.attendance,
                self.config.quiet_mode,
            );
            let result = executor
                .execute_source(&intent, &message, &row.source, row.ordinal, now)
                .await;
            if matches!(&result, Err(failure) if failure.attempt.is_some())
                || result.as_ref().is_ok_and(SendOutcome::claimed)
            {
                claimed += 1;
            }
            let outcome = settle(result)?;
            let drained = drains(&outcome);
            if drained {
                self.store
                    .mark_drained(lease, &row.source, row.ordinal, now)
                    .await?;
            }
            report.sends.push(NoticeSend {
                source: row.source,
                ordinal: row.ordinal,
                send: SendReport { intent, outcome },
                drained,
            });
        }
        Ok(report)
    }
}
