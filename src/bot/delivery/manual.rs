//! Manual header rewrites (`/debug rewrite`, `POST /api/admin/headers/rewrite`):
//! one run rewrites every header already posted this boss week — each bound
//! reminder card naming a run still ahead and the week's active digest —
//! and edits the posts in place, so nobody is pinged again, nothing is
//! uploaded and ✅/❌ and the journal binding stay.
//!
//! The classic style shows no countdown or digest phrase, so only day-of
//! headings are rewritten then. Calls are sequential through the shared
//! [`HeadingRewrite`](super::cards::HeadingRewrite) (each logged once in the
//! Rewrites log, stage `manual`) with the pre-generation deadline. An
//! accepted line is appended as an override (the original line is kept) and
//! the post is re-rendered by [`CardRefresh`]; a rejected or failed rewrite
//! stores and edits nothing. One run at a time: a trigger while one is
//! queued or running is refused. The worker ends a call in flight at stop
//! (logged as `shutdown`) and abandons the rest.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::{Notify, watch};

use super::card_records::digest_phrase_key;
use super::cards::{
    DAY_OF_KIND, DigestPhraseStore, HeaderOverrideStore, HeadingSource, PhraseKind, PostedCard,
    ReminderCardStore, card_runs, local_day,
};
use super::pregen::PREGEN_DEADLINE;
use super::refresh::CardRefresh;
use crate::bot::ids::parse_id;
use crate::bot::mentions;
use crate::bot::transport::{DiscordTransport, OutgoingMessage};
use crate::chat::nudge::{
    NudgeRewriter, RewriteFailure, RewriteOutcome, RewritePrompt, SharedRewriter,
};
use crate::domain::model_log::RewriteStage;
use crate::domain::notify::{DeliveryJournal, WeekReset, WeeklyDigest};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::domain::settings::MessageStyle;
use crate::runtime::logging;

/// Who asked for a run and where its summary goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualRequest {
    /// Recorded with each override: `member:<id>` or the admin session's actor.
    pub actor: String,
    /// A channel to post the summary in when the run ends (`/debug rewrite`).
    pub report_to: Option<String>,
}

/// The answer to a trigger, given before any model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManualStart {
    /// Queued: this many headers will be rewritten.
    Started(usize),
    /// Another manual run is queued or running.
    Running,
    /// No rewrite model or persona.
    Disabled,
    /// Nothing posted this boss week has a header to rewrite.
    Nothing,
    /// The store could not be read, or the worker has stopped.
    Unavailable,
}

/// What a finished run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ManualReport {
    /// Lines accepted and stored as overrides.
    pub accepted: usize,
    /// Replies the gate refused.
    pub rejected: usize,
    /// Calls that failed or timed out, and overrides that could not be stored.
    pub failed: usize,
    /// Accepted lines whose post could not be edited now (the next refresh shows them).
    pub unedited: usize,
    /// Cut short by shutdown.
    pub stopped: bool,
}

impl ManualReport {
    /// The short summary `/debug rewrite` posts.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "✏️ Rewrote this boss week's headers: {} accepted, {} rejected, {} failed.",
            self.accepted, self.rejected, self.failed
        );
        if self.unedited > 0 {
            text.push_str(&format!(
                " {} post(s) could not be edited now; they update on their next refresh.",
                self.unedited
            ));
        }
        if self.stopped {
            text.push_str(" Stopped early: the bot is shutting down.");
        }
        text
    }
}

enum Header {
    DayOf { day: String },
    Countdown,
}

enum Target {
    Card {
        card: PostedCard,
        key: String,
        header: Header,
    },
    Digest {
        digest: WeeklyDigest,
        key: String,
    },
}

impl Target {
    fn key(&self) -> &str {
        match self {
            Self::Card { key, .. } | Self::Digest { key, .. } => key,
        }
    }
}

struct Job {
    request: ManualRequest,
    targets: Vec<Target>,
}

/// Notes whether the model replied, which tells a refused line (rejected)
/// from a failed call: both come back from the rewrite as the seed.
struct Replied {
    inner: SharedRewriter,
    replied: Arc<AtomicBool>,
}

impl NudgeRewriter for Replied {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        self.rewrite_detailed(prompt, deadline).await.result
    }

    async fn rewrite_detailed(&self, prompt: &RewritePrompt, deadline: Duration) -> RewriteOutcome {
        let outcome = self.inner.rewrite_detailed(prompt, deadline).await;
        if outcome.result.is_ok() {
            self.replied.store(true, Ordering::Relaxed);
        }
        outcome
    }
}

/// The single-flight manual run: [`Self::start`] plans and queues it,
/// [`Self::run`] (one worker task) carries it out.
pub struct ManualRewrite<S, T> {
    refresh: Arc<CardRefresh<S, T>>,
    busy: AtomicBool,
    closed: AtomicBool,
    queued: Mutex<Option<Job>>,
    wake: Notify,
    finished: watch::Sender<Option<ManualReport>>,
}

impl<S, T> ManualRewrite<S, T>
where
    S: ScheduleStore
        + ReminderCardStore
        + DigestPhraseStore
        + HeaderOverrideStore
        + DeliveryJournal
        + Sync,
    T: DiscordTransport,
{
    /// Rewrites through `refresh`'s card kit and edits through it.
    pub fn new(refresh: Arc<CardRefresh<S, T>>) -> Self {
        Self {
            refresh,
            busy: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            queued: Mutex::new(None),
            wake: Notify::new(),
            finished: watch::channel(None).0,
        }
    }

    /// The last finished run's report, updated as each run ends.
    pub fn finished(&self) -> watch::Receiver<Option<ManualReport>> {
        self.finished.subscribe()
    }

    /// Plan a run for this boss week and queue it; never waits on the model.
    pub async fn start(&self, request: ManualRequest) -> ManualStart {
        if !self.refresh.cards.heading.enabled() {
            return ManualStart::Disabled;
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return ManualStart::Running;
        }
        let answer = if self.closed.load(Ordering::Acquire) {
            ManualStart::Unavailable
        } else {
            match self.plan((self.refresh.now)()).await {
                None => {
                    logging::event(
                        "WARN",
                        "manual_rewrite_failed",
                        json!({"operation": "plan"}),
                    );
                    ManualStart::Unavailable
                }
                Some(targets) if targets.is_empty() => ManualStart::Nothing,
                Some(targets) => {
                    let count = targets.len();
                    *self.queued.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Job { request, targets });
                    // The worker stopped while this was planned: it never
                    // runs, so take the job back (the lock orders this
                    // against the worker's final drain).
                    if self.closed.load(Ordering::Acquire) {
                        self.queued
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .take();
                        ManualStart::Unavailable
                    } else {
                        self.wake.notify_one();
                        logging::event("INFO", "manual_rewrite_started", json!({"headers": count}));
                        return ManualStart::Started(count);
                    }
                }
            }
        };
        self.busy.store(false, Ordering::Release);
        answer
    }

    /// Carry out queued runs until `stop`; a run in flight at stop ends its
    /// model call (logged as `shutdown`) and abandons the rest. Later
    /// triggers are refused.
    pub async fn run(&self, mut stop: watch::Receiver<bool>) {
        loop {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => break,
                () = self.wake.notified() => {}
            }
            let job = self
                .queued
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            let Some(job) = job else {
                continue;
            };
            let report = self.execute(&job, &stop).await;
            logging::event(
                "INFO",
                "manual_rewrite_done",
                json!({
                    "accepted": report.accepted,
                    "rejected": report.rejected,
                    "failed": report.failed,
                    "unedited": report.unedited,
                    "stopped": report.stopped,
                }),
            );
            if !report.stopped
                && let Some(channel) = job.request.report_to.as_deref().and_then(parse_id)
            {
                let summary = OutgoingMessage {
                    content: Some(report.summary()),
                    embeds: Vec::new(),
                    allowed_mentions: mentions::none(),
                    reply_to: None,
                    attachments: Vec::new(),
                };
                if !self
                    .refresh
                    .transport
                    .create_message(channel, &summary)
                    .await
                    .is_delivered()
                {
                    logging::event(
                        "WARN",
                        "manual_rewrite_failed",
                        json!({"operation": "summary"}),
                    );
                }
            }
            self.busy.store(false, Ordering::Release);
            self.finished.send_replace(Some(report));
            if *stop.borrow() {
                break;
            }
        }
        self.closed.store(true, Ordering::Release);
        // A run queued but never started is dropped, so later triggers are
        // refused as unavailable rather than as running.
        if self
            .queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .is_some()
        {
            self.busy.store(false, Ordering::Release);
        }
    }

    /// Every bound card of this boss week naming a run still ahead (day-of
    /// only in the classic style), earliest run first, then the week's
    /// active digest (redesigned only); `None` on a store error.
    async fn plan(&self, now: DateTime<Utc>) -> Option<Vec<Target>> {
        let refresh = &*self.refresh;
        let policy = &refresh.policy;
        let reset = WeekReset {
            zone: policy.zone(),
            weekday: policy.reset_weekday,
            time: policy.reset_time,
        };
        let week = reset.current_week(now).ok()?;
        let phrases = refresh.cards.style() == MessageStyle::Redesigned;
        let schedule = refresh.store.load(&Scope::All).await.ok()?;
        let mut runs: Vec<(DateTime<Utc>, String)> = schedule
            .runs
            .iter()
            .filter(|run| run.week_start == week && run.datetime > now)
            .map(|run| (run.datetime, run.id.clone()))
            .collect();
        runs.sort();
        let mut seen = HashSet::new();
        let mut posted = Vec::new();
        for (_, run_id) in &runs {
            for card in refresh.store.posted_cards(run_id).await.ok()? {
                if card.dedupe_key.is_some() && seen.insert(card.message_id.clone()) {
                    posted.push(card);
                }
            }
        }
        let mut targets = {
            let ctx = refresh.context(&schedule);
            posted
                .into_iter()
                .filter_map(|card| {
                    let header = if card.record.kind == DAY_OF_KIND {
                        let first = card_runs(&ctx, &card.run_ids).first()?.datetime;
                        Header::DayOf {
                            day: local_day(first, ctx.zone),
                        }
                    } else if phrases {
                        Header::Countdown
                    } else {
                        return None;
                    };
                    Some(Target::Card {
                        key: card.dedupe_key.clone()?,
                        card,
                        header,
                    })
                })
                .collect::<Vec<_>>()
        };
        if phrases {
            let log = refresh.store.load_digests().await.ok()?;
            let digest = log
                .digests
                .into_iter()
                .find(|digest| digest.week_start == week && digest.retired_at.is_none());
            if let (Some(digest), Some(key)) = (digest, digest_phrase_key(week)) {
                targets.push(Target::Digest { digest, key });
            }
        }
        Some(targets)
    }

    async fn execute(&self, job: &Job, stop: &watch::Receiver<bool>) -> ManualReport {
        let cards = &self.refresh.cards;
        let replied = Arc::new(AtomicBool::new(false));
        let mut heading = cards.heading.clone();
        heading.rewriter = heading.rewriter.map(|inner| {
            SharedRewriter(Arc::new(Replied {
                inner,
                replied: Arc::clone(&replied),
            }))
        });
        let catalog = cards.catalog.as_deref();
        let stage = RewriteStage::Manual;
        let mut report = ManualReport::default();
        for target in &job.targets {
            if *stop.borrow() {
                report.stopped = true;
                break;
            }
            replied.store(false, Ordering::Relaxed);
            let key = target.key();
            let chosen = match target {
                Target::Card {
                    header: Header::DayOf { day },
                    ..
                } => {
                    heading
                        .choose(day, key, stage, PREGEN_DEADLINE, Some(stop))
                        .await
                }
                Target::Card { .. } | Target::Digest { .. } => {
                    let kind = if matches!(target, Target::Digest { .. }) {
                        PhraseKind::Digest
                    } else {
                        PhraseKind::Countdown
                    };
                    heading
                        .choose_phrase(kind, catalog, key, stage, PREGEN_DEADLINE, Some(stop))
                        .await
                }
            };
            if chosen.source != HeadingSource::Rewrite {
                if chosen.code == Some("shutdown") {
                    report.stopped = true;
                    break;
                }
                if replied.load(Ordering::Relaxed) {
                    report.rejected += 1;
                } else {
                    report.failed += 1;
                }
                continue;
            }
            let at = (self.refresh.now)();
            if self
                .refresh
                .store
                .override_header(key, &chosen.line, &job.request.actor, at)
                .await
                .is_err()
            {
                logging::event(
                    "WARN",
                    "manual_rewrite_failed",
                    json!({"operation": "write"}),
                );
                report.failed += 1;
                continue;
            }
            report.accepted += 1;
            if !self.apply(target, chosen.line).await {
                report.unedited += 1;
            }
        }
        report
    }

    /// Edit the post with its new line through the refresh path.
    async fn apply(&self, target: &Target, line: String) -> bool {
        let Ok(schedule) = self.refresh.store.load(&Scope::All).await else {
            return false;
        };
        match target {
            Target::Card { card, .. } => {
                let mut card = card.clone();
                card.record.heading = Some(line);
                self.refresh.edit(&schedule, &card).await
            }
            Target::Digest { digest, .. } => self.refresh.edit_digest(&schedule, digest).await,
        }
    }
}
