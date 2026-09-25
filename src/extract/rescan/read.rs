//! One channel of a rescan (v4 `Pipeline.rescan_window`): backfill, read the
//! window's gated messages (widening an empty `week` once), one call per
//! conversation at the backlog's pace, then one consolidated proposal pass.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tokio::time::{Instant, sleep_until};

use super::{History, ResolvedWindow, TURNED_AWAY_ATTEMPTS};
use crate::domain::model_log::{MessageUpsert, ModelLogStore};
use crate::domain::scheduler::ScheduleStore;
use crate::domain::time::to_iso;
use crate::domain::weeks;
use crate::extract::pipeline::{CallRecord, Extractor, Failure, Outbox, Proposer};
use crate::extract::window::{
    BURST_GAP, MAX_BURST_MESSAGES, group_for_rescan, previous_week_start, should_widen,
    window_since,
};
use crate::infrastructure::llm::LlmProvider;

/// Spaces a job's model calls by the drain interval.
pub(super) struct Pace {
    interval: std::time::Duration,
    last: Option<Instant>,
}

impl Pace {
    pub fn new(interval: std::time::Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    async fn wait(&mut self, not_before: Option<Instant>) {
        let paced = self.last.map(|last| last + self.interval);
        if let Some(when) = paced.max(not_before) {
            sleep_until(when).await;
        }
        self.last = Some(Instant::now());
    }
}

/// The message ids of the turned-away pieces, and the latest retry time.
fn turned_away(records: &[CallRecord]) -> (HashSet<String>, Option<Instant>) {
    let mut ids = HashSet::new();
    let mut retry = None;
    for record in records {
        if let Some(Failure::TurnedAway { retry_at }) = record.failure {
            ids.extend(record.message_ids.iter().cloned());
            retry = retry.max(retry_at);
        }
    }
    (ids, retry)
}

pub(super) struct Reader<'a, S, P, X, O, H> {
    pub extractor: &'a Extractor<S, P, X, O>,
    pub history: &'a H,
    pub stop: &'a AtomicBool,
    pub pace: &'a mut Pace,
    /// The startup rescan: messages a pass already read are skipped.
    pub unprocessed_only: bool,
}

impl<S, P, X, O, H> Reader<'_, S, P, X, O, H>
where
    S: ScheduleStore + ModelLogStore + Send + Sync,
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
    H: History,
{
    /// Asked to stop, or extraction switched off (which stops the job).
    fn stopped(&self) -> bool {
        if !self.extractor.guild().extraction_enabled() {
            self.stop.store(true, Ordering::SeqCst);
        }
        self.stop.load(Ordering::SeqCst)
    }

    async fn backfill(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
        errors: &mut Vec<String>,
    ) -> usize {
        let messages = match self.history.backfill(channel_id, since).await {
            Ok(backfilled) => {
                errors.extend(backfilled.skipped);
                backfilled.messages
            }
            Err(error) => {
                // A rescan still reads what is cached.
                errors.push(format!("backfill: {error}"));
                return 0;
            }
        };
        let mut stored = 0;
        for message in &messages {
            match self.extractor.store_message(message).await {
                Ok(Some(MessageUpsert::Inserted | MessageUpsert::Edited)) => stored += 1,
                Ok(_) => {}
                Err(error) => errors.push(error.to_string()),
            }
        }
        stored
    }

    /// The per-channel result stored in the job's `results`; `Err` only when
    /// the channel could not be read at all.
    pub async fn read(
        &mut self,
        channel_id: &str,
        window: ResolvedWindow,
        automated: bool,
    ) -> Result<Value, String> {
        let config = self.extractor.config().clone();
        let now = self.extractor.now();
        let since = window_since(
            window.key,
            config.zone,
            config.reset_weekday,
            config.reset_time,
            &now.fixed_offset(),
        )
        .map_err(|error| error.to_string())?;
        let mut since = since.with_timezone(&Utc);
        let mut errors = Vec::new();
        let mut backfilled = self.backfill(channel_id, since, &mut errors).await;
        let (mut stored, mut gated) = self
            .extractor
            .gated_since(channel_id, since, self.unprocessed_only)
            .await
            .map_err(|error| error.to_string())?;
        let mut widened = false;
        if window.may_widen && should_widen(window.key, gated.len(), automated) {
            // A quiet week just after the reset: last week's plan, once only.
            let this =
                weeks::week_start(&since, config.zone, config.reset_weekday, config.reset_time)
                    .and_then(|this| {
                        previous_week_start(
                            &this,
                            config.zone,
                            config.reset_weekday,
                            config.reset_time,
                        )
                    })
                    .map_err(|error| error.to_string())?;
            widened = true;
            since = this.to_fixed().with_timezone(&Utc);
            backfilled += self.backfill(channel_id, since, &mut errors).await;
            (stored, gated) = self
                .extractor
                .gated_since(channel_id, since, self.unprocessed_only)
                .await
                .map_err(|error| error.to_string())?;
        }
        let groups = group_for_rescan(&gated, config.zone, BURST_GAP, MAX_BURST_MESSAGES, |row| {
            row.created_at
        });

        let mut records: Vec<CallRecord> = Vec::new();
        let mut cancelled = false;
        let mut unread = 0;
        'groups: for group in &groups {
            let mut not_before = None;
            // Only the pieces turned away are read again.
            let mut pending = group.clone();
            for _ in 0..TURNED_AWAY_ATTEMPTS {
                // Taken before the check so a switch-off between the two
                // still cuts the wait below.
                let mark = self.extractor.cut_mark();
                // Cooperative: a call in flight finishes, the next never starts.
                if self.stopped() {
                    cancelled = true;
                    break 'groups;
                }
                // A breaker wait can be long; shutdown or a switch-off cuts it.
                tokio::select! {
                    biased;
                    _ = self.extractor.cut(mark) => {}
                    () = self.pace.wait(not_before) => {}
                }
                if self.stopped() {
                    cancelled = true;
                    break 'groups;
                }
                let batch = self.extractor.call_burst(channel_id, pending.clone()).await;
                let (away, retry_at) = turned_away(&batch);
                records.extend(batch);
                pending.retain(|row| away.contains(&row.id));
                if pending.is_empty() {
                    break;
                }
                // Breaker open or rate-limited: wait for the governor.
                not_before = retry_at;
            }
            unread += pending.len();
        }
        if unread > 0 {
            errors.push(format!(
                "{unread} message(s) not read: the model kept turning the rescan away"
            ));
        }
        let extracted = records.iter().filter(|record| record.ok()).count();
        let calls = records.len();
        let report = self.extractor.commit_pass(channel_id, records).await;
        errors.extend(report.errors);
        Ok(json!({
            "channel_id": channel_id,
            "name": self.extractor.guild().channel_name(channel_id),
            "window": window.key,
            "since": to_iso(&since).ok(),
            "widened": widened,
            "backfilled": backfilled,
            "stored": stored,
            "gated": gated.len(),
            "bursts": groups.len(),
            "calls": calls,
            "extracted": extracted,
            "proposals": report.proposals.len(),
            "refused": report.refused.len(),
            "dropped": report.dropped,
            "stale": report.stale,
            "cancelled": cancelled,
            "unread": unread,
            "errors": errors,
        }))
    }
}
