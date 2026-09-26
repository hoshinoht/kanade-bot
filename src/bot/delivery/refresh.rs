//! Re-rendering posted cards after a run changes (v4 `card_needs_refresh`,
//! `refresh_run_cards`, `refresh_weekly_digest`). Every run write queues the
//! run ids ([`RefreshQueue`], fed by the store's run-write observer); one
//! task ([`CardRefresh::run`]) drains them in coalesced batches, off the
//! reaction worker and the tick. Per batch: each bound reminder card with a
//! record naming a run still ahead is edited from current answers (same
//! heading, art referenced by its posted names, nothing uploaded, nobody
//! notified), then the active digest of each touched week. Cards posted
//! before records existed (plain text) are left alone. `/debug ping` test
//! cards of day-of/countdown kind are refreshed too, keeping their prefix.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::{Notify, watch};

use super::cards::{
    self, CardArt, CardContext, CardKit, DAY_OF_KIND, PostedCard, ReminderCardStore, fetch_art,
};
use super::debug::{TEST_PREFIX, test_mentions};
use crate::bot::ids::parse_id;
use crate::bot::transport::DiscordTransport;
use crate::domain::attendance::{AttendanceMode, countdown_mentions, morning_mentions};
use crate::domain::members::Directory;
use crate::domain::notify::{
    DeliveryJournal, IntentContent, PingKind, countdown_minutes, digest_inclusion, everyone_on,
    resolve_mentions,
};
use crate::domain::schedule::{SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::runtime::logging;

pub type Now = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Distinct runs held for the next batch; more are dropped (and logged):
/// a stale tally is cosmetic, an unbounded queue is not.
pub const MAX_PENDING_RUNS: usize = 1024;

/// Runs whose posted cards need a re-render. Requests coalesce: a run
/// queued many times before the task wakes is refreshed once.
#[derive(Debug, Default)]
pub struct RefreshQueue {
    pending: Mutex<BTreeSet<String>>,
    wake: Notify,
}

impl RefreshQueue {
    /// Queue `run_ids`; cheap and non-blocking, safe from a store commit.
    pub fn request(&self, run_ids: &[String]) {
        if run_ids.is_empty() {
            return;
        }
        let mut dropped = 0;
        {
            let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
            for id in run_ids {
                if pending.len() < MAX_PENDING_RUNS || pending.contains(id) {
                    pending.insert(id.clone());
                } else {
                    dropped += 1;
                }
            }
        }
        if dropped > 0 {
            logging::event("WARN", "card_refresh_dropped", json!({"runs": dropped}));
        }
        self.wake.notify_one();
    }

    /// Everything queued so far, emptied.
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner))
            .into_iter()
            .collect()
    }
}

pub struct CardRefresh<S, T> {
    pub store: Arc<S>,
    pub transport: Arc<T>,
    pub members: Arc<dyn Directory + Send + Sync>,
    pub cards: CardKit,
    pub policy: SchedulePolicy,
    /// The tick's live quiet-mode setting.
    pub quiet: Arc<AtomicBool>,
    pub now: Now,
}

impl<S, T> CardRefresh<S, T>
where
    S: ScheduleStore + ReminderCardStore + DeliveryJournal + Sync,
    T: DiscordTransport,
{
    /// Drain `queue` until `stop` turns true; a batch in flight is abandoned
    /// at stop (edits are idempotent and nothing is journalled).
    pub async fn run(&self, queue: &RefreshQueue, mut stop: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => return,
                () = queue.wake.notified() => {}
            }
            loop {
                let batch = queue.take();
                if batch.is_empty() {
                    break;
                }
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stop| *stop) => return,
                    _ = self.refresh(&batch) => {}
                }
            }
        }
    }

    /// Edit the posted cards of `run_ids` and their weeks' digests; returns
    /// how many edits landed. Failures are skipped: a stale tally is cosmetic.
    pub async fn refresh(&self, run_ids: &[String]) -> usize {
        let now = (self.now)();
        let Ok(schedule) = self.store.load(&Scope::All).await else {
            return 0;
        };
        let mut seen = BTreeSet::new();
        let mut edited = 0;
        for run_id in run_ids {
            // v4: a run that has started keeps its cards as a record.
            let ahead = schedule
                .runs
                .iter()
                .any(|run| &run.id == run_id && run.datetime > now);
            if !ahead {
                continue;
            }
            let Ok(posted) = self.store.posted_cards(run_id).await else {
                continue;
            };
            for card in posted {
                if seen.insert(card.message_id.clone()) && self.edit(&schedule, &card).await {
                    edited += 1;
                }
            }
        }
        edited + self.refresh_digests(&schedule, run_ids).await
    }

    /// v4 `refresh_weekly_digest`: the active digest of each touched week,
    /// including runs that already happened (it records the week).
    async fn refresh_digests(&self, schedule: &ScheduleSnapshot, run_ids: &[String]) -> usize {
        let weeks: BTreeSet<DateTime<Utc>> = schedule
            .runs
            .iter()
            .filter(|run| run_ids.contains(&run.id))
            .map(|run| run.week_start)
            .collect();
        if weeks.is_empty() {
            return 0;
        }
        let Ok(log) = self.store.load_digests().await else {
            return 0;
        };
        let zone = self.policy.zone();
        let mut edited = 0;
        for digest in log
            .digests
            .iter()
            .filter(|digest| digest.retired_at.is_none() && weeks.contains(&digest.week_start))
        {
            let (Some(channel), Some(message), Ok(inclusion)) = (
                parse_id(&digest.channel_id),
                parse_id(&digest.message_id),
                digest_inclusion(&schedule.runs, digest.week_start, zone),
            ) else {
                continue;
            };
            let content = IntentContent::Digest {
                week_start: digest.week_start,
                inclusion,
            };
            let Some(card) = cards::build(&content, &self.context(schedule), None, &[]) else {
                continue;
            };
            let edit = card.edit(&CardArt::default());
            if self
                .transport
                .edit_message(channel, message, &edit)
                .await
                .is_delivered()
            {
                edited += 1;
            }
        }
        edited
    }

    fn context<'s>(&'s self, schedule: &'s ScheduleSnapshot) -> CardContext<'s> {
        CardContext {
            schedule,
            attendance: self.policy.attendance,
            zone: self.policy.zone(),
            quiet: self.quiet.load(Ordering::Relaxed),
            members: &*self.members,
            catalog: self.cards.catalog.as_deref(),
        }
    }

    async fn edit(&self, schedule: &ScheduleSnapshot, posted: &PostedCard) -> bool {
        let (Some(channel), Some(message)) =
            (parse_id(&posted.channel_id), parse_id(&posted.message_id))
        else {
            return false;
        };
        let content = if posted.record.kind == DAY_OF_KIND {
            IntentContent::DayOf {
                run_ids: posted.run_ids.clone(),
            }
        } else {
            let (Some(minutes), Some(run_id)) = (
                countdown_minutes(&posted.record.kind),
                posted.run_ids.first(),
            ) else {
                return false;
            };
            IntentContent::Countdown {
                run_id: run_id.clone(),
                minutes,
            }
        };
        let ctx = self.context(schedule);
        // A test card keeps its `test` audience and prefix (v4 `_rebuild_test_card`).
        let mentioned = if posted.test {
            match posted.run_ids.first().and_then(|id| ctx.run(id)) {
                Some(run) => test_mentions(&*self.members, run, ctx.quiet),
                None => return false,
            }
        } else {
            self.mentioned(&ctx, &content)
        };
        let Some(mut card) =
            cards::build(&content, &ctx, posted.record.heading.as_deref(), &mentioned)
        else {
            return false;
        };
        if posted.test {
            card.content = format!("{TEST_PREFIX}{}", card.content);
        }
        let pictures = fetch_art(self.cards.art.as_ref(), &card, false).await;
        let edit = card.edit(&pictures);
        self.transport
            .edit_message(channel, message, &edit)
            .await
            .is_delivered()
    }

    /// Who the card names as a mention, planned as dispatch plans it, so an
    /// edit reads like a fresh post (it notifies nobody either way).
    fn mentioned(&self, ctx: &CardContext<'_>, content: &IntentContent) -> Vec<String> {
        if ctx.quiet {
            return Vec::new();
        }
        let members: &dyn Directory = &*self.members;
        match content {
            IntentContent::DayOf { run_ids } => {
                let runs = cards::card_runs(ctx, run_ids);
                let candidates = match ctx.attendance.mode {
                    AttendanceMode::V4Compat => {
                        everyone_on(runs.iter().map(|run| run.participants.as_slice()))
                    }
                    AttendanceMode::V5 => {
                        let unknown: Vec<Vec<String>> = runs
                            .iter()
                            .map(|run| morning_mentions(&ctx.states(run)))
                            .collect();
                        everyone_on(unknown.iter().map(Vec::as_slice))
                    }
                };
                resolve_mentions(members, &candidates, &PingKind::DayOf)
            }
            IntentContent::Countdown { run_id, .. } => match ctx.run(run_id) {
                Some(run) => resolve_mentions(
                    members,
                    &countdown_mentions(&ctx.states(run)),
                    &PingKind::Countdown,
                ),
                None => Vec::new(),
            },
            _ => Vec::new(),
        }
    }
}
