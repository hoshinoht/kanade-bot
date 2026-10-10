//! Re-rendering posted cards after a run changes (v4 `card_needs_refresh`,
//! `refresh_run_cards`, `refresh_weekly_digest`). Every run write queues the
//! run ids ([`RefreshQueue`], fed by the store's run-write observer); one
//! task ([`CardRefresh::run`]) drains them in coalesced batches, off the
//! reaction worker and the tick. Per batch: each bound reminder card with a
//! record naming a run still ahead is edited from current answers (same
//! heading or phrase, the latest manual override when there is one; art
//! referenced by its posted names, nothing uploaded, nobody notified), then
//! the active digest of each touched week. Cards posted
//! before records existed (plain text) are left alone. `/debug ping` test
//! cards of day-of/countdown kind are refreshed too, keeping their prefix.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};

use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::{Mutex as AsyncMutex, Notify, watch};

use super::card_records;
use super::cards::redesign::learn_after_refusal;
use super::cards::{
    self, CardArt, CardContext, CardKit, DAY_OF_KIND, DigestPhraseStore, PostedCard,
    ReminderCardStore, fetch_art,
};
use super::debug::{TEST_PREFIX, test_mentions};
use crate::bot::ids::parse_id;
use crate::bot::transport::DiscordTransport;
use crate::domain::attendance::{AttendanceMode, countdown_mentions, morning_mentions};
use crate::domain::members::Directory;
use crate::domain::notify::{
    DeliveryJournal, IntentContent, PingKind, WeeklyDigest, countdown_minutes, digest_inclusion,
    everyone_on, resolve_mentions,
};
use crate::domain::schedule::{SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::runtime::logging;

pub type Now = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Distinct runs held for the next batch; more are dropped (and logged):
/// a stale tally is cosmetic, an unbounded queue is not.
pub const MAX_PENDING_RUNS: usize = 1024;

static EDIT_LOCKS: LazyLock<Mutex<HashMap<String, Weak<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One edit at a time per posted message: the refresh worker and a manual
/// rewrite each re-read the stored line under it, so the later edit always
/// shows the latest override and a stale render never lands after it. The
/// proposal card desk takes it too, so a button press and a reaction never
/// land their renders out of order.
pub(crate) fn edit_lock(message_id: &str) -> Arc<AsyncMutex<()>> {
    let mut locks = EDIT_LOCKS.lock().unwrap_or_else(PoisonError::into_inner);
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(message_id).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(message_id.to_owned(), Arc::downgrade(&lock));
    lock
}

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
    S: ScheduleStore + ReminderCardStore + DigestPhraseStore + DeliveryJournal + Sync,
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
        let mut edited = 0;
        for digest in log
            .digests
            .iter()
            .filter(|digest| digest.retired_at.is_none() && weeks.contains(&digest.week_start))
        {
            if self.edit_digest(schedule, digest).await {
                edited += 1;
            }
        }
        edited
    }

    /// Re-render a posted digest from current runs and its stored phrase
    /// (the latest manual override, else the original), read under the
    /// message's edit lock; whether it landed.
    ///
    /// A redesigned digest is sent as Components V2, which converts a live
    /// legacy digest on the first refresh after the style flips. Discord
    /// cannot take the flag off again: while a V2 digest is live and the
    /// card renders as an embed (the style flipped back to classic, or the
    /// week outgrew the V2 budget), the digest is left as posted until next
    /// week's replaces it, noted once in the log. A V2 digest this process
    /// has not seen yet is found out by Discord refusing the embed edit.
    pub(super) async fn edit_digest(
        &self,
        schedule: &ScheduleSnapshot,
        digest: &WeeklyDigest,
    ) -> bool {
        let ends = self.cards.run_ends(&self.policy);
        let now = (self.now)();
        let (Some(channel), Some(message), Ok(inclusion)) = (
            parse_id(&digest.channel_id),
            parse_id(&digest.message_id),
            digest_inclusion(
                &schedule.runs,
                digest.week_start,
                self.policy.zone(),
                ends.as_ref().map(|ends| (ends, now)),
            ),
        ) else {
            return false;
        };
        let content = IntentContent::Digest {
            week_start: digest.week_start,
            inclusion,
        };
        let Some(key) = card_records::digest_phrase_key(digest.week_start) else {
            return false;
        };
        let lock = edit_lock(&digest.message_id);
        let _held = lock.lock().await;
        let phrase = match self.store.digest_phrase(&key).await {
            Ok(phrase) => phrase,
            Err(_) => {
                logging::event("WARN", "digest_phrase_failed", json!({"operation": "read"}));
                return false;
            }
        };
        let Some(card) = cards::build(&content, &self.context(schedule), phrase.as_deref(), &[])
        else {
            return false;
        };
        let formats = &self.cards.v2.formats;
        let v2 = !card.components.is_empty();
        if !v2 && formats.is_v2(&digest.message_id) {
            self.left_as_posted(digest);
            return false;
        }
        let edit = card.edit(&CardArt::default());
        let outcome = self.transport.edit_message(channel, message, &edit).await;
        if outcome.is_delivered() {
            formats.record(&digest.message_id, v2);
            return true;
        }
        if !v2
            && learn_after_refusal(
                formats,
                &*self.transport,
                &digest.channel_id,
                &digest.message_id,
                &outcome,
            )
            .await
        {
            self.left_as_posted(digest);
        }
        false
    }

    /// A live V2 digest that renders as an embed now stays as posted.
    fn left_as_posted(&self, digest: &WeeklyDigest) {
        if self.cards.v2.formats.first_note(&digest.message_id) {
            logging::event(
                "INFO",
                "digest_v2_left_as_posted",
                json!({
                    "week_start": digest.week_start.timestamp(),
                    "style": self.cards.style().as_str(),
                    "until": "next week's digest",
                }),
            );
        }
    }

    pub(super) fn context<'s>(&'s self, schedule: &'s ScheduleSnapshot) -> CardContext<'s> {
        CardContext {
            schedule,
            attendance: self.policy.attendance,
            zone: self.policy.zone(),
            quiet: self.quiet.load(Ordering::Relaxed),
            members: &*self.members,
            catalog: self.cards.catalog.as_deref(),
            style: self.cards.style(),
            marks: &self.cards.marks,
            v2: Some(&self.cards.v2),
        }
    }

    /// Re-render a posted card from current answers with its record's
    /// heading, re-read under the message's edit lock (a manual override
    /// written since `posted` was listed wins); whether the edit landed.
    pub(super) async fn edit(&self, schedule: &ScheduleSnapshot, posted: &PostedCard) -> bool {
        let (Some(channel), Some(message)) =
            (parse_id(&posted.channel_id), parse_id(&posted.message_id))
        else {
            return false;
        };
        let Some(content) = posted_content(posted) else {
            return false;
        };
        let ctx = self.context(schedule);
        // A test card keeps its `test` audience and prefix (v4 `_rebuild_test_card`).
        let mentioned = if posted.test {
            match posted.run_ids.first().and_then(|id| ctx.run(id)) {
                Some(run) => test_mentions(&*self.members, run, ctx.quiet),
                None => return false,
            }
        } else {
            posted_mentions(&ctx, &content)
        };
        let lock = edit_lock(&posted.message_id);
        let _held = lock.lock().await;
        let heading = match &posted.dedupe_key {
            Some(key) => match self.store.card_record(key).await {
                Ok(Some(record)) => record.heading,
                Ok(None) => posted.record.heading.clone(),
                Err(_) => return false,
            },
            None => posted.record.heading.clone(),
        };
        let Some(mut card) = cards::build(&content, &ctx, heading.as_deref(), &mentioned) else {
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
}

/// What a posted card shows: its record's kind over the runs it was posted
/// for (its card→run rows), never the runs' current grouping.
pub fn posted_content(posted: &PostedCard) -> Option<IntentContent> {
    if posted.record.kind == DAY_OF_KIND {
        return Some(IntentContent::DayOf {
            run_ids: posted.run_ids.clone(),
        });
    }
    Some(IntentContent::Countdown {
        run_id: posted.run_ids.first()?.clone(),
        minutes: countdown_minutes(&posted.record.kind)?,
    })
}

/// Who the card names as a mention, planned as dispatch plans it, so an
/// edit reads like a fresh post (it notifies nobody either way).
pub fn posted_mentions(ctx: &CardContext<'_>, content: &IntentContent) -> Vec<String> {
    if ctx.quiet {
        return Vec::new();
    }
    let members: &dyn Directory = ctx.members;
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
