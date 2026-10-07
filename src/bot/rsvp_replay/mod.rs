//! Bounded recovery of RSVP-card reactions after a fresh Discord READY.

mod plan;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::json;
use twilight_model::id::{
    Id,
    marker::{ChannelMarker, MessageMarker, UserMarker},
};

use crate::api::auth::Clock as WallClock;
use crate::bot::cards::ReplayLive;
use crate::bot::delivery::{AdminAlert, AlertSink, AlertThrottle};
use crate::bot::events::{ReplayCard, ReplayCards, ReplayRunCards};
use crate::bot::ids::parse_id;
use crate::bot::reaction_read;
use crate::bot::transport::DiscordTransport;
use crate::domain::history::{
    Actor, BlameIndex, BlameTarget, ChangeHistory, Expect, Origin, Precondition, RowKey, RowValue,
    Surface, Versioned, rsvp_field,
};
use crate::domain::ids::RandomIds;
use crate::domain::schedule::{Rsvp, RsvpSource, Run, RunStatus};
use crate::domain::scheduler::{Clock, ScheduleStore, SchedulerError, SchedulerService};
use crate::runtime::logging;

const MAX_AGE: TimeDelta = TimeDelta::days(7);
const MAX_MESSAGES: usize = 40;
const MAX_CARDS_PER_RUN: usize = 8;
const MAX_RSVP_REACTION_PAGES: usize = 2;
/// Initial snapshot plus this many fresh attempts after a live event.
const MAX_LIVE_REACTION_RETRIES: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayReport {
    pub messages: usize,
    pub applied: usize,
    pub skipped: usize,
    pub conflicts: usize,
    pub requests: usize,
    pub aborted: bool,
}

struct ServiceClock(WallClock);

impl Clock for ServiceClock {
    fn now(&self) -> DateTime<Utc> {
        (self.0)()
    }
}

/// The single history/store view that authorizes one run's replay writes. It
/// is deliberately captured before any remote read and never refreshed during
/// the attempt: changes while HTTP is in flight must become `StaleEdit`.
struct PreparedRun {
    run: Run,
    cards: Vec<ReplayCard>,
    view: Versioned,
}

enum ApplyRun {
    Done,
    Restart,
}

struct ApplyInput<'a, L> {
    message_ids: &'a BTreeSet<String>,
    current: &'a (dyn Fn() -> bool + Send + Sync),
    live: &'a mut L,
    cache: &'a mut BTreeMap<String, plan::CardAnswer>,
    report: &'a mut ReplayReport,
}

/// The RSVP replay's dependencies are deliberately independent from proposal
/// cards: this path reads only the journal, history and Discord reactions.
pub struct RsvpReplay<S, T, A> {
    store: Arc<S>,
    transport: Arc<T>,
    clock: WallClock,
    alerts: Arc<A>,
    throttle: AlertThrottle,
}

impl<S, T, A> RsvpReplay<S, T, A>
where
    S: ScheduleStore + ChangeHistory + BlameIndex + ReplayCards + Send + Sync + 'static,
    T: DiscordTransport,
    A: AlertSink,
{
    pub fn new(store: Arc<S>, transport: Arc<T>, clock: WallClock, alerts: Arc<A>) -> Self {
        Self {
            store,
            transport,
            clock,
            alerts,
            throttle: AlertThrottle::new(),
        }
    }

    pub async fn replay(
        &mut self,
        self_id: Id<UserMarker>,
        current: impl Fn() -> bool + Send + Sync,
        live: &mut impl ReplayLive,
    ) -> ReplayReport {
        let started = Instant::now();
        let at = (self.clock)();
        let candidates = match self.store.replay_cards(at - MAX_AGE).await {
            Ok(cards) => cards,
            Err(_) => return self.aborted(ReplayReport::default(), started, "cards"),
        };
        let mut report = ReplayReport::default();
        let mut permission_failures = 0usize;
        let mut planned = BTreeSet::new();
        let mut ordered = Vec::new();
        for cards in candidates {
            let prepared = match self.prepare(cards).await {
                Ok(Some(prepared)) => prepared,
                Ok(None) => continue,
                Err(()) => return self.aborted(report, started, "versioned"),
            };
            if eligible(&prepared.run, at) {
                ordered.push(prepared);
            }
        }
        ordered.sort_by(|left, right| {
            left.run
                .datetime
                .cmp(&right.run.datetime)
                .then(left.run.id.cmp(&right.run.id))
        });
        let mut cache = BTreeMap::new();
        for prepared in ordered {
            if !current() {
                report.aborted = true;
                break;
            }
            let ids: BTreeSet<_> = prepared
                .cards
                .iter()
                .map(|card| card.message_id.clone())
                .collect();
            if prepared.cards.len() > MAX_CARDS_PER_RUN {
                report.skipped += 1;
                skipped(&prepared.run.id, "card_cap");
                continue;
            }
            let added = ids.difference(&planned).count();
            if planned.len() + added > MAX_MESSAGES {
                report.skipped += 1;
                skipped(&prepared.run.id, "budget");
                continue;
            }
            planned.extend(ids.iter().cloned());
            report.messages = planned.len();
            let mut completed = false;
            for attempt in 0..=MAX_LIVE_REACTION_RETRIES {
                let touched = live.drain().await;
                invalidate(&mut cache, &touched);
                if !current() {
                    report.aborted = true;
                    break;
                }
                if !touched.is_disjoint(&ids) {
                    if attempt == MAX_LIVE_REACTION_RETRIES {
                        report.skipped += 1;
                        skipped(&prepared.run.id, "live_reaction_retry_cap");
                        break;
                    }
                    continue;
                }
                let Some((answers, requests, permissions)) = self
                    .read_run(&prepared, self_id, &current, &mut cache)
                    .await
                else {
                    report.aborted = true;
                    break;
                };
                report.requests += requests;
                permission_failures += permissions;
                let touched = live.drain().await;
                invalidate(&mut cache, &touched);
                if !current() {
                    report.aborted = true;
                    break;
                }
                if !touched.is_disjoint(&ids) {
                    if attempt == MAX_LIVE_REACTION_RETRIES {
                        report.skipped += 1;
                        skipped(&prepared.run.id, "live_reaction_retry_cap");
                        break;
                    }
                    continue;
                }
                match self
                    .apply_run(
                        &prepared,
                        &answers,
                        ApplyInput {
                            message_ids: &ids,
                            current: &current,
                            live,
                            cache: &mut cache,
                            report: &mut report,
                        },
                    )
                    .await
                {
                    Ok(ApplyRun::Done) => {
                        completed = true;
                        break;
                    }
                    Ok(ApplyRun::Restart) if attempt < MAX_LIVE_REACTION_RETRIES => continue,
                    Ok(ApplyRun::Restart) => {
                        report.skipped += 1;
                        skipped(&prepared.run.id, "live_reaction_retry_cap");
                        break;
                    }
                    Err(()) => {
                        report.aborted = true;
                        break;
                    }
                }
            }
            if report.aborted {
                break;
            }
            if !completed {
                // A partial/incomplete card read deliberately leaves the run
                // untouched; the per-card reason was already logged.
            }
        }
        if permission_failures > 0 {
            let alert = AdminAlert::RsvpReplayPermissionDenied {
                messages: permission_failures,
            };
            if self.throttle.admit(&alert, (self.clock)()) {
                self.alerts.alert(alert);
            }
        }
        finished(&report, started);
        report
    }

    fn aborted(&self, mut report: ReplayReport, started: Instant, reason: &str) -> ReplayReport {
        report.aborted = true;
        logging::event("WARN", "rsvp_replay_aborted", json!({"reason": reason}));
        finished(&report, started);
        report
    }

    async fn prepare(&self, cards: ReplayRunCards) -> Result<Option<PreparedRun>, ()> {
        let target = BlameTarget::Run(cards.run_id);
        let mut views = self
            .store
            .read_versioned(std::slice::from_ref(&target))
            .await
            .map_err(|_| ())?;
        let view = views.pop().ok_or(())?;
        let Some(RowValue::Run(run)) = view.row.clone() else {
            return Ok(None);
        };
        let slot_at = match view.versions.get("slot") {
            Some(seq) => {
                self.store
                    .load_change(*seq)
                    .await
                    .map_err(|_| ())?
                    .ok_or(())?
                    .at
            }
            None => DateTime::UNIX_EPOCH,
        };
        let cards = cards
            .cards
            .into_iter()
            .map(|mut card| {
                card.evidence &= card.resolved_at.is_some_and(|bound| bound >= slot_at);
                card
            })
            .collect::<Vec<_>>();
        Ok(cards
            .iter()
            .any(|card| card.evidence)
            .then_some(PreparedRun { run, cards, view }))
    }

    /// Read only cache misses. A cancellation/error is terminal for the pass;
    /// a completed HTTP failure is cached as incomplete and logged per card.
    async fn read_run(
        &self,
        prepared: &PreparedRun,
        self_id: Id<UserMarker>,
        current: &impl Fn() -> bool,
        cache: &mut BTreeMap<String, plan::CardAnswer>,
    ) -> Option<(Vec<plan::CardAnswer>, usize, usize)> {
        let mut answers = Vec::with_capacity(prepared.cards.len());
        let mut requests = 0;
        let mut permissions = 0;
        for card in &prepared.cards {
            if !current() {
                return None;
            }
            if let Some(cached) = cache.get(&card.message_id) {
                let mut answer = cached.clone();
                answer.evidence = card.evidence;
                answers.push(answer);
                continue;
            }
            let (Some(channel), Some(message)) = (
                parse_id::<ChannelMarker>(card.channel_id.as_deref().unwrap_or_default()),
                parse_id::<MessageMarker>(&card.message_id),
            ) else {
                skipped_card(&prepared.run.id, &card.message_id, "invalid_id");
                let answer = plan::CardAnswer {
                    message_id: card.message_id.clone(),
                    evidence: card.evidence,
                    ..Default::default()
                };
                cache.insert(card.message_id.clone(), answer.clone());
                answers.push(answer);
                continue;
            };
            let answer = match reaction_read::read_answers(
                &*self.transport,
                channel,
                message,
                self_id,
                MAX_RSVP_REACTION_PAGES,
                current,
            )
            .await
            {
                Ok(read) => {
                    requests += read.requests;
                    plan::CardAnswer {
                        message_id: card.message_id.clone(),
                        evidence: card.evidence,
                        complete: true,
                        yes: read.yes.into_iter().map(|id| id.to_string()).collect(),
                        no: read.no.into_iter().map(|id| id.to_string()).collect(),
                        own_yes: read.own_yes,
                        own_no: read.own_no,
                    }
                }
                Err(error) if error.reason == "stale_generation" => return None,
                Err(error) => {
                    requests += error.requests;
                    permissions += usize::from(
                        error.reason == "missing_access" || error.reason == "missing_permissions",
                    );
                    skipped_card(&prepared.run.id, &card.message_id, &error.reason);
                    plan::CardAnswer {
                        message_id: card.message_id.clone(),
                        evidence: card.evidence,
                        ..Default::default()
                    }
                }
            };
            cache.insert(card.message_id.clone(), answer.clone());
            answers.push(answer);
        }
        Some((answers, requests, permissions))
    }

    async fn apply_run<L: ReplayLive>(
        &self,
        prepared: &PreparedRun,
        cards: &[plan::CardAnswer],
        input: ApplyInput<'_, L>,
    ) -> Result<ApplyRun, ()> {
        let target = BlameTarget::Run(prepared.run.id.clone());
        for user in &prepared.run.participants {
            let remote = plan::remote(cards, user);
            if remote == plan::RemoteDecision::Conflict {
                input.report.conflicts += 1;
                logging::event(
                    "WARN",
                    "rsvp_replay_conflict",
                    json!({"run_id": prepared.run.id, "user_id": user}),
                );
                continue;
            }
            let stored = prepared.view.answers.iter().find_map(|row| match row {
                RowValue::Rsvp(rsvp) if rsvp.user_id == *user => Some(rsvp.clone()),
                _ => None,
            });
            let field = rsvp_field(user);
            let seen = prepared.view.versions.get(&field).copied();
            let owned = self
                .reaction_owned(&prepared.run.id, user, stored.as_ref(), seen)
                .await?;
            let (state, added) = match remote {
                plan::RemoteDecision::Answer(state)
                    if plan::evidence_complete(cards) && plan::has_evidence(cards, user, state) =>
                {
                    (state, true)
                }
                plan::RemoteDecision::None
                    if stored
                        .as_ref()
                        .is_some_and(|rsvp| rsvp.source == RsvpSource::Reaction)
                        && plan::removal_guard(
                            cards,
                            user,
                            stored.as_ref().expect("checked").state,
                        ) =>
                {
                    (stored.as_ref().expect("checked").state, false)
                }
                _ => continue,
            };
            if !owned || (added && stored.as_ref().is_some_and(|rsvp| rsvp.state == state)) {
                continue;
            }
            if !(input.current)() {
                return Err(());
            }
            let touched = input.live.drain().await;
            invalidate(input.cache, &touched);
            if !touched.is_disjoint(input.message_ids) {
                return Ok(ApplyRun::Restart);
            }
            // This must stay adjacent to the commit: time can pass while the
            // reaction worker drains its queued live events.
            if (self.clock)() >= prepared.run.datetime {
                skipped(&prepared.run.id, "started");
                continue;
            }
            if !(input.current)() {
                return Err(());
            }
            let emoji = match state.as_str() {
                "yes" => "✅",
                "no" => "❌",
                _ => return Err(()),
            };
            let message = cards
                .iter()
                .find(|card| {
                    card.complete
                        && match state.as_str() {
                            "yes" => card.yes.contains(user),
                            "no" => card.no.contains(user),
                            _ => false,
                        }
                })
                .or_else(|| cards.first())
                .map(|card| card.message_id.as_str())
                .unwrap_or("none");
            let request = format!(
                "rsvp-replay:{}:{}:{}:{}:{}",
                message,
                prepared.run.id,
                emoji,
                added,
                seen.map_or_else(|| "none".into(), |seq| seq.to_string())
            );
            let expect = Expect::fields([
                Precondition::new(target.clone(), field, seen),
                Precondition::new(
                    target.clone(),
                    "slot",
                    prepared.view.versions.get("slot").copied(),
                ),
                Precondition::new(
                    target.clone(),
                    "status",
                    prepared.view.versions.get("status").copied(),
                ),
            ]);
            let mut service = SchedulerService::new(
                Arc::clone(&self.store),
                RandomIds,
                ServiceClock(Arc::clone(&self.clock)),
            );
            match service
                .as_origin(
                    Origin::new(Actor::member(user), Surface::Discord).with_request_id(request),
                )
                .expecting(expect)
                .apply_reaction(&prepared.run.id, user, emoji, added)
                .await
            {
                Ok(result) if result.applied => {
                    input.report.applied += 1;
                    logging::event(
                        "INFO",
                        "rsvp_replay_applied",
                        json!({"run_id": prepared.run.id, "user_id": user, "added": added}),
                    );
                }
                Err(SchedulerError::StaleEdit { .. }) => {
                    input.report.skipped += 1;
                    skipped(&prepared.run.id, "stale_edit");
                }
                Err(_) => return Err(()),
                Ok(_) => {}
            }
        }
        Ok(ApplyRun::Done)
    }

    async fn reaction_owned(
        &self,
        run_id: &str,
        user: &str,
        stored: Option<&Rsvp>,
        seen: Option<u64>,
    ) -> Result<bool, ()> {
        let Some(seq) = seen else {
            // A history-less answer can be newly added from remote state, but
            // never cleared: there is no proof that it came from a reaction.
            return Ok(stored.is_none());
        };
        let record = self
            .store
            .load_change(seq)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        let actor = record.origin.actor == Actor::member(user);
        let request = record.origin.request_id.as_deref();
        let origin = actor
            && record.origin.surface == Surface::Discord
            && request.is_none_or(|id| id.starts_with("rsvp-replay:"));
        if !origin {
            return Ok(false);
        }
        match stored {
            Some(rsvp) => Ok(rsvp.source == RsvpSource::Reaction),
            None => Ok(record.rows.iter().any(|row| {
                row.key
                    == RowKey::Rsvp {
                        run_id: run_id.to_owned(),
                        user_id: user.to_owned(),
                    }
                    && row.before.is_some()
                    && row.after.is_none()
            })),
        }
    }
}

fn invalidate(cache: &mut BTreeMap<String, plan::CardAnswer>, touched: &BTreeSet<String>) {
    for message in touched {
        cache.remove(message);
    }
}

fn eligible(run: &Run, now: DateTime<Utc>) -> bool {
    matches!(
        run.status,
        RunStatus::Planned | RunStatus::Confirmed | RunStatus::AtRisk
    ) && now < run.datetime
}

fn skipped(run_id: &str, reason: &str) {
    logging::event(
        "INFO",
        "rsvp_replay_skipped",
        json!({"run_id": run_id, "reason": reason}),
    );
}

fn skipped_card(run_id: &str, message_id: &str, reason: &str) {
    logging::event(
        "WARN",
        "rsvp_replay_skipped",
        json!({"run_id": run_id, "message_id": message_id, "reason": reason}),
    );
}

fn finished(report: &ReplayReport, started: Instant) {
    logging::event(
        if report.aborted { "WARN" } else { "INFO" },
        if report.aborted {
            "rsvp_replay_aborted"
        } else {
            "rsvp_replay_complete"
        },
        json!({"messages": report.messages, "applied": report.applied, "skipped": report.skipped, "conflicts": report.conflicts, "requests": report.requests, "duration_ms": started.elapsed().as_millis()}),
    );
}
