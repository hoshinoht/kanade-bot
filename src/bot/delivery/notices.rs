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
//!
//! A notice older than `max_notice_age` is retired `stale` unsent (a backlog
//! written while nothing drained must not flood the channels). Within one
//! source, a notice left pending (released, failed or over the cap) holds
//! back the source's later ordinals for the rest of the tick, so a merge's
//! requester notice never overtakes its summary. Undecodable rows stay
//! pending and are alerted, never fatal. The outbox is read in keyset pages
//! (`NOTICE_PAGE` rows), so a backlog is never held in memory whole.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use super::alerts::{AdminAlert, AlertSink};
use super::cards::ReminderCardStore;
use super::executor::{SendOutcome, SendReport};
use super::notice_text::render_notice;
use super::tick::{Delivery, DeliveryError, TickLease, settle};
use crate::bot::transport::DiscordTransport;
use crate::domain::drafts::ProposalStore;
use crate::domain::history::Checkpoints;
use crate::domain::notify::{DeliveryJournal, DrainReason, NoticeOutbox, plan_notice};
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
    /// retried next tick until it goes stale.
    pub unroutable: usize,
    /// Not attempted because the per-tick cap was reached.
    pub deferred: usize,
    /// Held back behind an earlier notice of the same source still pending.
    pub waiting: usize,
    /// Retired unsent: older than `max_notice_age`.
    pub stale: usize,
    /// Drained unsent: nothing to post any more (e.g. its run is gone).
    pub silent: usize,
    /// Pending rows whose stored payload does not decode; left pending.
    pub undecodable: usize,
}

/// Released sends never reached Discord and a failed journal write is
/// retried by claim; everything else is final for the outbox.
fn drains(outcome: &SendOutcome) -> bool {
    !matches!(outcome, SendOutcome::Released(_) | SendOutcome::Failed(_))
}

/// Pending rows read per page. The drain still visits the whole backlog each
/// call (stale retirement and the counts are not capped), but holds only one
/// page of it; the cap on claims is `max_sends_per_tick`, not this.
const NOTICE_PAGE: usize = 100;

impl<S, I, T, A> Delivery<'_, S, I, T, A>
where
    S: ScheduleStore
        + DeliveryJournal
        + NoticeOutbox
        + Checkpoints
        + ProposalStore
        + ReminderCardStore
        + Sync,
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
        self.leased(now, async move |this: &mut Self, lease: &TickLease| {
            this.notices_in(lease, now).await
        })
        .await
    }

    /// Send pending notices in write order, at most `max_sends_per_tick`
    /// claims per call. Every pending row is still visited each call (stale
    /// ones retire past the cap, the rest are counted), one page of
    /// [`NOTICE_PAGE`] rows in memory at a time.
    pub(super) async fn notices_in(
        &self,
        lease: &TickLease,
        now: DateTime<Utc>,
    ) -> Result<NoticeReport, DeliveryError> {
        let mut report = NoticeReport::default();
        let limit = self.config.max_sends_per_tick;
        let zone = self.config.policy.zone();
        let mut loaded = None;
        let mut claimed = 0;
        let mut held_back = BTreeSet::new();
        let mut after = None;
        'drain: loop {
            let page = self.store.pending_notices_after(after, NOTICE_PAGE).await?;
            report.undecodable += page.undecodable.len();
            for bad in page.undecodable {
                self.raise(
                    AdminAlert::NoticeUndecodable {
                        source: bad.source,
                        ordinal: bad.ordinal,
                        detail: bad.detail,
                    },
                    now,
                );
            }
            for row in page.notices {
                if now - row.created_at > self.config.max_notice_age {
                    let drained = self
                        .in_operation(async || {
                            let lease = self.lease(lease).await?;
                            self.store
                                .mark_drained(
                                    lease,
                                    &row.source,
                                    row.ordinal,
                                    DrainReason::Stale,
                                    now,
                                )
                                .await
                        })
                        .await;
                    let Some(drained) = drained else {
                        break 'drain;
                    };
                    drained?;
                    report.stale += 1;
                    continue;
                }
                if held_back.contains(&row.source) {
                    report.waiting += 1;
                    continue;
                }
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
                    held_back.insert(row.source);
                    continue;
                }
                // Loaded once per call, and only when a notice gets this far.
                let schedule = match &loaded {
                    Some(schedule) => schedule,
                    None => &*loaded.insert(self.store.load(&Scope::All).await?),
                };
                let Some(message) = render_notice(
                    &row.notice,
                    &intent,
                    schedule,
                    self.members,
                    zone,
                    self.config.quiet_mode,
                    &self.cards,
                ) else {
                    let drained = self
                        .in_operation(async || {
                            let lease = self.lease(lease).await?;
                            self.store
                                .mark_drained(
                                    lease,
                                    &row.source,
                                    row.ordinal,
                                    DrainReason::Silent,
                                    now,
                                )
                                .await
                        })
                        .await;
                    let Some(drained) = drained else {
                        break 'drain;
                    };
                    drained?;
                    report.silent += 1;
                    continue;
                };
                let lease = self.lease(lease).await?;
                let Some(mut operation) = self.admit().await else {
                    break 'drain;
                };
                let result = self
                    .executor(lease)
                    .execute_source_in_operation(
                        &mut operation,
                        &intent,
                        &message,
                        &row.source,
                        row.ordinal,
                        now,
                    )
                    .await;
                let result = match result {
                    Ok(Some(outcome)) => Ok(outcome),
                    Ok(None) => break 'drain,
                    Err(failure) => Err(failure),
                };
                if matches!(&result, Err(failure) if failure.attempt.is_some())
                    || result.as_ref().is_ok_and(SendOutcome::claimed)
                {
                    claimed += 1;
                }
                let outcome = match settle(result) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        operation.settle();
                        return Err(error);
                    }
                };
                let drained = if drains(&outcome) {
                    let result = self
                        .store
                        .mark_drained(lease, &row.source, row.ordinal, DrainReason::Journal, now)
                        .await;
                    operation.settle();
                    result?;
                    true
                } else {
                    operation.settle();
                    false
                };
                if !drained {
                    held_back.insert(row.source.clone());
                }
                report.sends.push(NoticeSend {
                    source: row.source,
                    ordinal: row.ordinal,
                    send: SendReport { intent, outcome },
                    drained,
                });
            }
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        if report.stale > 0 {
            self.raise(
                AdminAlert::StaleNoticesRetired {
                    count: report.stale,
                },
                now,
            );
        }
        Ok(report)
    }
}
