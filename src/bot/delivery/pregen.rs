//! Reminder header pre-generation: the persona rewrite of each upcoming
//! card's day-of heading or countdown/digest phrase, stored before its send
//! under the key the send path reads (`card_records`).
//!
//! The intents come from the admin preview's planner (`plan_dispatch` at each
//! reminder's fire time), so grouping and keys match the tick's. Writes are
//! insert-if-absent and no lock is held across the model call: a seed stored
//! by a send first always wins, and a stored line never changes afterwards.
//! A failed rewrite stores nothing; the key is retried on later passes up to
//! [`MAX_ATTEMPTS_PER_KEY`] times, after which the send uses the seed.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
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
use crate::domain::notify::{
    DeliveryJournal, DeliverySettings, DeliveryTarget, DigestLog, IntentContent, JournalView,
    WeekReset,
};
use crate::domain::schedule::{Reminder, SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::domain::time::from_iso;
use crate::runtime::logging;

/// Cards firing within this window are pre-generated.
pub const PREGEN_HORIZON: TimeDelta = TimeDelta::hours(12);
/// One rewrite's budget; within the governor's 300 s session cap.
pub const PREGEN_DEADLINE: Duration = Duration::from_secs(30);
/// How often a pass runs.
pub const PREGEN_INTERVAL: Duration = Duration::from_secs(60);
/// Sequential rewrites per pass; the rest wait for the next pass.
pub const MAX_REWRITES_PER_PASS: usize = 4;
/// Failed rewrites per key in this process before it is left to the seed.
pub const MAX_ATTEMPTS_PER_KEY: u32 = 3;

/// What one pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PregenReport {
    /// Rewrites stored.
    pub stored: usize,
    /// Rewrites that failed, timed out or were rejected (nothing stored).
    pub failed: usize,
    /// Rewrites whose key a send stored first.
    pub lost: usize,
    /// Keys left for a later pass by the per-pass cap.
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

/// The pre-generation worker. A rewriter and persona must both be set,
/// otherwise a pass does nothing and sends keep the seed.
pub struct HeaderPregen<S> {
    pub store: Arc<S>,
    pub members: Arc<dyn Directory + Send + Sync>,
    pub cards: CardKit,
    pub policy: SchedulePolicy,
    pub now: Now,
    attempts: Mutex<HashMap<String, u32>>,
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
            attempts: Mutex::new(HashMap::new()),
        }
    }

    /// A pass every [`PREGEN_INTERVAL`] until `stop`; at stop a pass in
    /// flight abandons its model call and its remaining keys, never a store
    /// write.
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

    /// One pass at the clock's reading: rewrite at most
    /// [`MAX_REWRITES_PER_PASS`] missing headers, earliest first.
    pub async fn pass(&self) -> PregenReport {
        self.pass_until(None).await
    }

    async fn pass_until(&self, stop: Option<&watch::Receiver<bool>>) -> PregenReport {
        let mut report = PregenReport::default();
        if !self.cards.heading.enabled() {
            return report;
        }
        let now = (self.now)();
        let Some(candidates) = self.candidates(now).await else {
            logging::event("WARN", "header_pregen_failed", json!({"operation": "plan"}));
            return report;
        };
        let keys: HashSet<&str> = candidates.iter().map(|c| c.key.as_str()).collect();
        self.attempts().retain(|key, _| keys.contains(key.as_str()));
        let mut calls = 0;
        for candidate in &candidates {
            if stop.is_some_and(|stop| *stop.borrow()) {
                break;
            }
            if self.attempts().get(&candidate.key).copied().unwrap_or(0) >= MAX_ATTEMPTS_PER_KEY {
                continue;
            }
            // Stored (or unreadable): leave it to the send path.
            if !matches!(self.missing(candidate).await, Some(true)) {
                continue;
            }
            if calls == MAX_REWRITES_PER_PASS {
                report.deferred += 1;
                continue;
            }
            calls += 1;
            match self.generate(candidate, stop).await {
                Some(true) => report.stored += 1,
                Some(false) => report.lost += 1,
                None => {
                    report.failed += 1;
                    *self.attempts().entry(candidate.key.clone()).or_default() += 1;
                }
            }
        }
        report
    }

    fn attempts(&self) -> std::sync::MutexGuard<'_, HashMap<String, u32>> {
        self.attempts.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Unsent cards firing in `(now, now + horizon]` and no journal claim,
    /// plus the next boss week's digest when its reset is in the window and
    /// that week has no digest yet; earliest first.
    async fn candidates(&self, now: DateTime<Utc>) -> Option<Vec<Candidate>> {
        let schedule = self.store.load(&Scope::All).await.ok()?;
        let view = self.store.load_view().await.ok()?;
        let until = now + PREGEN_HORIZON;
        let mut upcoming: Vec<&Reminder> = schedule
            .reminders
            .iter()
            .filter(|row| row.sent_at.is_none() && row.fire_at > now && row.fire_at <= until)
            .collect();
        upcoming.sort_by(|a, b| (a.fire_at, &a.id).cmp(&(b.fire_at, &b.id)));
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for reminder in upcoming {
            let Some(candidate) = self.reminder_candidate(&schedule, reminder, &view) else {
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
        let next = reset.current_week(until).ok()?;
        if next > reset.current_week(now).ok()? {
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
            IntentContent::Countdown { .. } => Header::Countdown { kind },
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

    /// Rewrite and store insert-if-absent: `Some(true)` stored, `Some(false)`
    /// a send stored first, `None` no rewrite (or the write failed).
    async fn generate(
        &self,
        candidate: &Candidate,
        stop: Option<&watch::Receiver<bool>>,
    ) -> Option<bool> {
        let heading = &self.cards.heading;
        let catalog = self.cards.catalog.as_deref();
        let chosen = async {
            match &candidate.header {
                Header::DayOf { day } => heading.choose(day, PREGEN_DEADLINE).await,
                Header::Countdown { .. } => {
                    heading
                        .choose_phrase(PhraseKind::Countdown, catalog, PREGEN_DEADLINE)
                        .await
                }
                Header::Digest { .. } => {
                    heading
                        .choose_phrase(PhraseKind::Digest, catalog, PREGEN_DEADLINE)
                        .await
                }
            }
        };
        // Stop cuts only the model call; the store work below finishes unless serve
        // aborts the worker at its shutdown cutoff, which rolls the save back whole.
        let (line, source) = match stop {
            Some(stop) => {
                let mut stop = stop.clone();
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stop| *stop) => return None,
                    chosen = chosen => chosen,
                }
            }
            None => chosen.await,
        };
        if source != HeadingSource::Rewrite {
            return None;
        }
        // A send may have claimed the card during the call; its line (stored
        // or, after a failed record write, unsaved) must stay the posted one.
        if self.claimed(candidate).await? {
            return Some(false);
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
            Ok(won) => Some(won),
            Err(_) => {
                logging::event(
                    "WARN",
                    "header_pregen_failed",
                    json!({"operation": "write"}),
                );
                None
            }
        }
    }
}
