//! Reminder header pre-generation: the persona rewrite of each upcoming
//! card's day-of heading or countdown/digest phrase, stored before its send
//! under the key the send path reads (`card_records`). Classic countdown and
//! digest cards show no phrase (v4), so while the live message style is
//! classic only day-of headings are rewritten.
//!
//! A daily batch at the configured guild-local time
//! (`notifications.header_generation_time`, read live) rewrites every card
//! firing within [`HEADER_HORIZON`], plus the coming week's digest when its
//! reset falls in that window. Between batches a [`PREGEN_INTERVAL`] tick
//! catches up on cards the last batch had not seen (runs added or moved
//! since, re-grouped cards) once they fire within the horizon, and retries
//! the batch's own failures. Until a process has run its first batch it
//! has no batch state, so catch-up takes only cards firing by the next
//! occurrence (the previous batch's window, which may have been missed) and
//! leaves later ones to that batch. A process whose batch time has passed
//! today without a batch runs one at once; a changed time applies from its
//! next occurrence and never re-generates stored lines.
//!
//! The intents come from the admin preview's planner (`plan_dispatch` at each
//! reminder's fire time), so grouping and keys match the tick's. Writes are
//! insert-if-absent and no lock is held across the model call: a seed stored
//! by a send first always wins, and a stored line never changes afterwards.
//! A failed rewrite stores nothing and is retried up to
//! [`MAX_ATTEMPTS_PER_KEY`] times; a busy rewrite permit (a governor
//! refusal) is retried on later ticks without spending an attempt, at most
//! [`MAX_BUSY_RETRIES`] times. After that the send uses the seed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;

use super::card_records::{digest_phrase_key, record_kind};
use super::cards::{
    CardContext, CardKit, CardRecord, DAY_OF_KIND, DigestPhraseStore, HeadingSource, PhraseKind,
    ReminderCardStore, card_runs, local_day,
};
use super::preview::{record_key, reminder_intent};
use super::refresh::Now;
use crate::domain::members::Directory;
use crate::domain::model_log::RewriteStage;
use crate::domain::notify::{
    DeliveryJournal, DeliverySettings, DeliveryTarget, DigestLog, IntentContent, JournalView,
    WeekReset,
};
use crate::domain::schedule::{Reminder, SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::domain::settings::MessageStyle;
use crate::domain::time::from_iso;
use crate::runtime::logging;

/// Cards firing within this window are rewritten (by a batch or catch-up).
pub const HEADER_HORIZON: TimeDelta = TimeDelta::hours(24);
/// How far ahead a batch records the cards it has seen, so catch-up leaves
/// them to the next batch.
pub const SEEN_HORIZON: TimeDelta = TimeDelta::days(8);
/// One rewrite's budget; within the governor's 300 s session cap.
pub const PREGEN_DEADLINE: Duration = Duration::from_secs(30);
/// How often the worker checks for a due batch and catches up.
pub const PREGEN_INTERVAL: Duration = Duration::from_secs(60);
/// Sequential catch-up rewrites per tick (a batch has no cap); the rest wait.
pub const MAX_CATCHUP_PER_TICK: usize = 4;
/// Failed rewrites per key in this process before it is left to the seed.
pub const MAX_ATTEMPTS_PER_KEY: u32 = 3;
/// Busy-permit retries per key before it is left to the seed.
pub const MAX_BUSY_RETRIES: u32 = 30;

/// The configured batch time, read per tick (`None` source: 00:00).
pub type HeaderTime = Arc<dyn Fn() -> NaiveTime + Send + Sync>;

/// What one tick did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PregenReport {
    /// A daily batch ran.
    pub batch: bool,
    /// Rewrites stored.
    pub stored: usize,
    /// Rewrites that failed, timed out or were rejected (nothing stored).
    pub failed: usize,
    /// Rewrites whose key a send stored first.
    pub lost: usize,
    /// Catch-up keys left for a later tick by the per-tick cap.
    pub deferred: usize,
}

enum Header {
    DayOf { day: String },
    Countdown { kind: String },
    Digest { week: DateTime<Utc> },
}

struct Candidate {
    at: DateTime<Utc>,
    key: String,
    targets: Vec<DeliveryTarget>,
    header: Header,
}

/// A digest week that has neither a posted card nor a marker at or past it.
fn digest_open(log: &DigestLog, week: DateTime<Utc>) -> bool {
    let recorded = log
        .last_digest_week
        .as_deref()
        .and_then(|text| from_iso(text).ok())
        .is_some_and(|last| last >= week);
    // A posted (even legacy, phrase-less) digest keeps the line it was posted with.
    !recorded && !log.digests.iter().any(|digest| digest.week_start == week)
}

/// Governor refusals of a try-only rewrite: the permit or rate token was
/// not free right now, so the key is retried without spending an attempt.
fn busy(code: Option<&str>) -> bool {
    matches!(code, Some("busy" | "rate_ceiling" | "backend_unavailable"))
}

/// Today's batch instant in `zone` (01:00 when the time falls in a DST gap).
fn occurrence(now: DateTime<Utc>, time: NaiveTime, zone: Tz) -> Option<DateTime<Utc>> {
    occurrence_on(now.with_timezone(&zone).date_naive(), time, zone)
}

fn occurrence_on(day: NaiveDate, time: NaiveTime, zone: Tz) -> Option<DateTime<Utc>> {
    [time, time + TimeDelta::hours(1)]
        .into_iter()
        .find_map(|at| zone.from_local_datetime(&day.and_time(at)).earliest())
        .map(|at| at.with_timezone(&Utc))
}

/// The first batch occurrence after `now`: today's, else tomorrow's.
fn next_occurrence(now: DateTime<Utc>, time: NaiveTime, zone: Tz) -> Option<DateTime<Utc>> {
    let today = now.with_timezone(&zone).date_naive();
    occurrence_on(today, time, zone)
        .filter(|at| *at > now)
        .or_else(|| occurrence_on(today.succ_opt()?, time, zone))
}

#[derive(Default)]
struct Plan {
    /// The occurrence the last batch ran for.
    last_batch: Option<DateTime<Utc>>,
    /// The configured time as last read, and when it last changed.
    time: Option<NaiveTime>,
    changed_at: Option<DateTime<Utc>>,
    /// Every card key the last batch saw, within [`SEEN_HORIZON`].
    seen: HashSet<String>,
    /// The last batch's keys still to store (failed, busy or cut by stop).
    pending: HashSet<String>,
    attempts: HashMap<String, u32>,
    busy: HashMap<String, u32>,
}

enum Done {
    Stored,
    Lost,
    Failed { busy: bool },
    Stopped,
}

/// The pre-generation worker. A rewriter and persona must both be set,
/// otherwise a tick does nothing and sends keep the seed.
pub struct HeaderPregen<S> {
    pub store: Arc<S>,
    pub members: Arc<dyn Directory + Send + Sync>,
    pub cards: CardKit,
    pub policy: SchedulePolicy,
    pub now: Now,
    time: Option<HeaderTime>,
    plan: Mutex<Plan>,
}

impl<S> HeaderPregen<S>
where
    S: ScheduleStore + DeliveryJournal + ReminderCardStore + DigestPhraseStore + Send + Sync,
{
    pub fn new(
        store: Arc<S>,
        members: Arc<dyn Directory + Send + Sync>,
        cards: CardKit,
        policy: SchedulePolicy,
        now: Now,
    ) -> Self {
        Self {
            store,
            members,
            cards,
            policy,
            now,
            time: None,
            plan: Mutex::new(Plan::default()),
        }
    }

    /// Read the batch time live (the Config setting); unset is 00:00.
    #[must_use]
    pub fn with_time(mut self, time: HeaderTime) -> Self {
        self.time = Some(time);
        self
    }

    /// A tick every [`PREGEN_INTERVAL`] until `stop`; at stop a tick in
    /// flight ends its model call (logged as `shutdown`) and abandons its
    /// remaining keys, never a store write.
    pub async fn run(&self, mut stop: watch::Receiver<bool>) {
        if !self.cards.heading.enabled() {
            return;
        }
        let mut interval = tokio::time::interval(PREGEN_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => return,
                _ = interval.tick() => {}
            }
            self.pass_until(Some(&stop)).await;
            if *stop.borrow() {
                return;
            }
        }
    }

    /// One tick at the clock's reading: the daily batch when it is due,
    /// else catch-up and the last batch's retries.
    pub async fn pass(&self) -> PregenReport {
        self.pass_until(None).await
    }

    fn plan(&self) -> std::sync::MutexGuard<'_, Plan> {
        self.plan.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether today's batch is due at `now`, noting a changed time first.
    fn batch_due(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let time = self.time.as_ref().map_or(NaiveTime::MIN, |time| time());
        let mut plan = self.plan();
        if plan.time.is_some_and(|seen| seen != time) {
            plan.changed_at = Some(now);
        }
        plan.time = Some(time);
        let at = occurrence(now, time, self.policy.zone())?;
        let due = at <= now
            && plan.last_batch.is_none_or(|last| last < at)
            && plan.changed_at.is_none_or(|changed| changed < at);
        due.then_some(at)
    }

    async fn pass_until(&self, stop: Option<&watch::Receiver<bool>>) -> PregenReport {
        let mut report = PregenReport::default();
        if !self.cards.heading.enabled() {
            return report;
        }
        let now = (self.now)();
        let batch = self.batch_due(now);
        let Some(candidates) = self.candidates(now, now + SEEN_HORIZON).await else {
            logging::event("WARN", "header_pregen_failed", json!({"operation": "plan"}));
            return report;
        };
        let due: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| candidate.at <= now + HEADER_HORIZON)
            .collect();
        // Until this process has run a batch, catch-up takes only cards the
        // previous batch would have covered (due by the next occurrence).
        let catchup_until = {
            let mut plan = self.plan();
            if let Some(at) = batch {
                plan.last_batch = Some(at);
                plan.seen = candidates.iter().map(|c| c.key.clone()).collect();
                plan.pending = due.iter().map(|c| c.key.clone()).collect();
                plan.busy.clear();
            }
            let keys: HashSet<&str> = candidates.iter().map(|c| c.key.as_str()).collect();
            plan.attempts.retain(|key, _| keys.contains(key.as_str()));
            plan.busy.retain(|key, _| keys.contains(key.as_str()));
            plan.pending.retain(|key| keys.contains(key.as_str()));
            match (plan.last_batch, plan.time) {
                (None, Some(time)) => next_occurrence(now, time, self.policy.zone()),
                _ => None,
            }
        };
        report.batch = batch.is_some();
        let mut calls = 0;
        for candidate in due {
            if stop.is_some_and(|stop| *stop.borrow()) {
                break;
            }
            let stage = {
                let plan = self.plan();
                let spent = plan.attempts.get(&candidate.key).copied().unwrap_or(0)
                    >= MAX_ATTEMPTS_PER_KEY
                    || plan.busy.get(&candidate.key).copied().unwrap_or(0) >= MAX_BUSY_RETRIES;
                if spent {
                    continue;
                }
                if batch.is_some() || plan.pending.contains(&candidate.key) {
                    RewriteStage::Batch
                } else if plan.seen.contains(&candidate.key)
                    || catchup_until.is_some_and(|until| candidate.at > until)
                {
                    // Seen by the last batch beyond its window, or past the
                    // next occurrence before any batch: that batch's.
                    continue;
                } else {
                    RewriteStage::Catchup
                }
            };
            // Stored (or unreadable): leave it to the send path.
            match self.missing(candidate).await {
                Some(true) => {}
                Some(false) => {
                    self.plan().pending.remove(&candidate.key);
                    continue;
                }
                None => continue,
            }
            if batch.is_none() {
                if calls == MAX_CATCHUP_PER_TICK {
                    report.deferred += 1;
                    continue;
                }
                calls += 1;
            }
            let done = self.generate(candidate, stage, stop).await;
            let mut plan = self.plan();
            match done {
                Done::Stored => {
                    report.stored += 1;
                    plan.pending.remove(&candidate.key);
                }
                Done::Lost => {
                    report.lost += 1;
                    plan.pending.remove(&candidate.key);
                }
                Done::Failed { busy } => {
                    report.failed += 1;
                    let count = if busy {
                        &mut plan.busy
                    } else {
                        &mut plan.attempts
                    };
                    *count.entry(candidate.key.clone()).or_default() += 1;
                }
                Done::Stopped => {}
            }
        }
        report
    }

    /// Unsent cards firing in `(now, until]` and no journal claim, plus the
    /// coming boss week's digest when that week has no digest yet; earliest
    /// first. Countdowns and the digest only in the redesigned style.
    async fn candidates(&self, now: DateTime<Utc>, until: DateTime<Utc>) -> Option<Vec<Candidate>> {
        let schedule = self.store.load(&Scope::All).await.ok()?;
        let view = self.store.load_view().await.ok()?;
        // Read live, as the cards do: classic shows no countdown/digest phrase.
        let phrases = self.cards.style() == MessageStyle::Redesigned;
        let mut upcoming: Vec<&Reminder> = schedule
            .reminders
            .iter()
            .filter(|row| row.sent_at.is_none() && row.fire_at > now && row.fire_at <= until)
            .collect();
        upcoming.sort_by(|a, b| (a.fire_at, &a.id).cmp(&(b.fire_at, &b.id)));
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for reminder in upcoming {
            let Some(candidate) = self.reminder_candidate(&schedule, reminder, &view, phrases)
            else {
                continue;
            };
            if seen.insert(candidate.key.clone()) {
                out.push(candidate);
            }
        }
        let reset = WeekReset {
            zone: self.policy.zone(),
            weekday: self.policy.reset_weekday,
            time: self.policy.reset_time,
        };
        // The week starting at the first reset after `now`.
        let next = reset.current_week(now + TimeDelta::days(7)).ok()?;
        if phrases && next > reset.current_week(now).ok()? && next <= until {
            let log = self.store.load_digests().await.ok()?;
            if digest_open(&log, next)
                && let Some(key) = digest_phrase_key(next)
            {
                out.push(Candidate {
                    at: next,
                    key,
                    targets: vec![DeliveryTarget::Digest(next)],
                    header: Header::Digest { week: next },
                });
            }
        }
        out.sort_by_key(|candidate| candidate.at);
        Some(out)
    }

    fn reminder_candidate(
        &self,
        schedule: &ScheduleSnapshot,
        reminder: &Reminder,
        view: &impl JournalView,
        phrases: bool,
    ) -> Option<Candidate> {
        // Only targets and grouping matter here, not channels or mentions.
        let settings = DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
            attendance: self.policy.attendance,
        };
        let intent = reminder_intent(schedule, &reminder.id, &*self.members, settings)?;
        if intent.targets.iter().any(|target| view.holds(target)) {
            return None;
        }
        let key = record_key(&intent)?;
        let kind = record_kind(&intent.content)?;
        let header = match &intent.content {
            IntentContent::DayOf { run_ids } => {
                let ctx = CardContext {
                    schedule,
                    attendance: self.policy.attendance,
                    zone: self.policy.zone(),
                    quiet: false,
                    members: &*self.members,
                    catalog: self.cards.catalog.as_deref(),
                    style: self.cards.style(),
                    marks: &self.cards.marks,
                };
                let first = card_runs(&ctx, run_ids).first()?.datetime;
                Header::DayOf {
                    day: local_day(first, ctx.zone),
                }
            }
            IntentContent::Countdown { .. } if phrases => Header::Countdown { kind },
            _ => return None,
        };
        Some(Candidate {
            at: reminder.fire_at,
            key,
            targets: intent.targets,
            header,
        })
    }

    /// `Some(true)` when nothing is stored under the key; `None` on a read error.
    async fn missing(&self, candidate: &Candidate) -> Option<bool> {
        match candidate.header {
            Header::Digest { .. } => self
                .store
                .digest_phrase(&candidate.key)
                .await
                .ok()
                .map(|phrase| phrase.is_none()),
            _ => self
                .store
                .card_record(&candidate.key)
                .await
                .ok()
                .map(|record| record.is_none()),
        }
    }

    /// Whether the card was claimed, posted or retired since it was planned; `None`
    /// on a read error.
    async fn claimed(&self, candidate: &Candidate) -> Option<bool> {
        let view = self.store.load_view().await.ok()?;
        if candidate.targets.iter().any(|target| view.holds(target)) {
            return Some(true);
        }
        match candidate.header {
            Header::Digest { week } => {
                let log = self.store.load_digests().await.ok()?;
                Some(!digest_open(&log, week))
            }
            // A bound send no longer holds its target but marks its rows sent.
            Header::DayOf { .. } | Header::Countdown { .. } => {
                let schedule = self.store.load(&Scope::All).await.ok()?;
                Some(candidate.targets.iter().any(|target| {
                    match target {
                        DeliveryTarget::Reminder(id) => !schedule
                            .reminders
                            .iter()
                            .any(|row| &row.id == id && row.sent_at.is_none()),
                        _ => false,
                    }
                }))
            }
        }
    }

    /// Rewrite and store insert-if-absent.
    async fn generate(
        &self,
        candidate: &Candidate,
        stage: RewriteStage,
        stop: Option<&watch::Receiver<bool>>,
    ) -> Done {
        let heading = &self.cards.heading;
        let catalog = self.cards.catalog.as_deref();
        let key = candidate.key.as_str();
        // Stop ends only the model call, inside the trial so its row is logged
        // (`shutdown`); the store work below finishes unless serve aborts the
        // worker at its shutdown cutoff, which rolls the save back whole.
        let chosen = match &candidate.header {
            Header::DayOf { day } => heading.choose(day, key, stage, PREGEN_DEADLINE, stop).await,
            Header::Countdown { .. } => {
                heading
                    .choose_phrase(
                        PhraseKind::Countdown,
                        catalog,
                        key,
                        stage,
                        PREGEN_DEADLINE,
                        stop,
                    )
                    .await
            }
            Header::Digest { .. } => {
                heading
                    .choose_phrase(
                        PhraseKind::Digest,
                        catalog,
                        key,
                        stage,
                        PREGEN_DEADLINE,
                        stop,
                    )
                    .await
            }
        };
        if chosen.source != HeadingSource::Rewrite {
            // A call the stop cut (code `shutdown`): no attempt spent, the key
            // stays pending. Any other failure counts, even if the stop has
            // fired since the call returned.
            if chosen.code == Some("shutdown") {
                return Done::Stopped;
            }
            return Done::Failed {
                busy: busy(chosen.code),
            };
        }
        let line = chosen.line;
        // A send may have claimed the card during the call; its line (stored
        // or, after a failed record write, unsaved) must stay the posted one.
        match self.claimed(candidate).await {
            Some(true) => return Done::Lost,
            Some(false) => {}
            None => return Done::Failed { busy: false },
        }
        let at = (self.now)();
        let saved = match &candidate.header {
            Header::Digest { .. } => self
                .store
                .save_digest_phrase(&candidate.key, &line, at)
                .await
                .map(|stored| stored == line),
            Header::DayOf { .. } | Header::Countdown { .. } => {
                let kind = match &candidate.header {
                    Header::Countdown { kind } => kind.clone(),
                    _ => DAY_OF_KIND.to_owned(),
                };
                let record = CardRecord {
                    kind,
                    heading: Some(line),
                };
                self.store
                    .save_card_record(&candidate.key, &record, at)
                    .await
                    .map(|stored| stored == record)
            }
        };
        match saved {
            Ok(true) => Done::Stored,
            Ok(false) => Done::Lost,
            Err(_) => {
                logging::event(
                    "WARN",
                    "header_pregen_failed",
                    json!({"operation": "write"}),
                );
                Done::Failed { busy: false }
            }
        }
    }
}
