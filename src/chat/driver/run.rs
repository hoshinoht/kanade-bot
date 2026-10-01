//! One channel's worker: answer the question, then hand over to the next
//! waiting one until the channel's queue is empty. A prepared question
//! always concludes, even when its future is dropped (a panic or a shutdown
//! abort), settling any clean-retry reservation: see [`Held`].

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use tokio::sync::watch;
use tokio::time::Instant;

use super::{Answerer, Asked, ChatDriver, ChatEvent, Job, Prepared, Queued, State, Surface};
use crate::chat::answer::{AnswerFailure, AnswerSettings, Generation, Question};
use crate::chat::context::{assemble, build_turns, system_prompt};
use crate::chat::gate::{CHANNEL_BUSY_REACTION, SEEN_REACTION, is_chat_channel};
use crate::chat::pilot::{ChatPilot, Concluded, Finished, LogFacts, ReplyPort, failure_reply};
use crate::chat::sanitize::schedule_defaults;
use crate::chat::tools::ToolContext;
use crate::infrastructure::llm::governor::{Charge, SessionError, SessionFailure};

use super::position_reaction;

const DELETED: &str = "cancelled: the question was deleted";
const CUT: &str = "cancelled: serve shut down";
const FAILED: &str = "failed: the question stopped unexpectedly";

/// The reply already went out (or was withheld); `conclude` only reads it.
struct Posted(Result<String, String>);

impl ReplyPort for Posted {
    fn post_reply(
        &self,
        _channel_id: &str,
        _reply_to: &str,
        _text: &str,
    ) -> impl Future<Output = Result<String, String>> + Send {
        std::future::ready(self.0.clone())
    }
}

/// Run a future that never waits (`conclude` over [`Posted`]) while the
/// state lock is held; a lock guard must not live across an await. `None`
/// if it did wait.
fn at_once<F: Future>(future: F) -> Option<F::Output> {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => Some(output),
        Poll::Pending => None,
    }
}

/// A question that ended without an answer and before any charge: refunded.
fn ended() -> Generation {
    Generation::failed(AnswerFailure::Session(SessionError {
        failure: SessionFailure::Ended,
        charge: Charge::Refunded,
    }))
}

async fn cut_signal(mut cut: watch::Receiver<bool>) {
    while !*cut.borrow_and_update() {
        if cut.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// Polls a future, turning a panic into `Err(())` (the task keeps running,
/// so the worker can still hand the channel over).
struct CatchPanic<F>(Pin<Box<F>>);

impl<F: Future> Future for CatchPanic<F> {
    type Output = Result<F::Output, ()>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let inner = self.0.as_mut();
        match catch_unwind(AssertUnwindSafe(|| inner.poll(cx))) {
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(Err(())),
        }
    }
}

fn tool_context(asked: &Asked, prepared: &Prepared, source_id: &str) -> ToolContext {
    let mut ctx = ToolContext::new(
        asked.message.author_id.clone(),
        asked.origin_id.clone(),
        asked.message.id.clone(),
        prepared.now,
    );
    // Proposals are recorded against the chat interaction.
    ctx.source_id = source_id.to_owned();
    ctx.is_admin = asked.is_admin;
    ctx.bot_user_id.clone_from(&asked.bot_user_id);
    ctx.self_role_id.clone_from(&asked.self_role_id);
    ctx.bot_names.clone_from(&prepared.bot_names);
    let defaults = schedule_defaults(
        &asked.message.content,
        asked.bot_user_id.as_deref(),
        asked.self_role_id.as_deref(),
    );
    ctx.force_all_channels = defaults.force_all_channels;
    ctx.force_channel_scope = defaults.force_channel_scope;
    ctx.force_group_schedule = defaults.force_group_schedule;
    ctx.self_schedule_requested = defaults.self_schedule_requested;
    ctx.upcoming_only = defaults.upcoming_only;
    ctx
}

/// A prepared question, armed before its context is built and holding its
/// clean-retry reservation once taken (`reserved`). Concluding it disarms
/// it; dropped armed (panic, abort) it concludes as a refunded failure, so
/// any reservation is settled and nothing is posted.
struct Held<A: Answerer, S: Surface> {
    driver: ChatDriver<A, S>,
    asked: Asked,
    prepared: Arc<Prepared>,
    ctx: ToolContext,
    row_id: String,
    spent_at: Option<f64>,
    reserved: bool,
    started: Instant,
    armed: bool,
    /// Already handed off from an unwinding drop: never defer again.
    deferred: bool,
}

impl<A: Answerer, S: Surface> Held<A, S> {
    /// `conclude` under the state lock. A deleted question and its unposted
    /// answer are withheld first, so neither reaches later context.
    fn conclude(
        &mut self,
        generation: &Generation,
        posted: Result<String, String>,
        deleted: bool,
    ) -> Option<Concluded> {
        let latency_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let now = self.driver.now();
        let mut guard = self.driver.state();
        let pilot = &mut guard.pilot;
        let asked = &self.asked;
        let posted = if deleted {
            let unposted = format!("unposted:{}", asked.message.id);
            pilot.conversations.withhold(&asked.message.id);
            pilot.conversations.withhold(&unposted);
            Ok(unposted)
        } else {
            posted
        };
        let prepared = &*self.prepared;
        let done = Finished {
            message: &asked.message,
            channel_id: &asked.origin_id,
            ctx: &self.ctx,
            generation,
            persona: &prepared.persona,
            directory: &*prepared.directory,
            log: LogFacts {
                id: self.row_id.clone(),
                at: prepared.now,
                model: &prepared.model,
                reasoning: prepared.reasoning,
                latency_ms,
            },
            spent_at: self.spent_at,
            reserved: self.reserved,
            now,
        };
        let concluded = at_once(pilot.conclude(done, &Posted(posted)));
        // Disarmed only once `conclude` returned: a panic inside it leaves
        // the guard armed, so its drop still settles and refunds.
        self.armed = false;
        if concluded.is_none() {
            // What `conclude` would have settled, so nothing leaks.
            let member = &asked.message.author_id;
            if self.reserved {
                pilot.guard.settle(member, false, now);
            }
            if let Some(stamp) = self.spent_at {
                pilot.allowance.refund(member, stamp);
            }
        }
        concluded
    }
}

impl<A: Answerer, S: Surface> Held<A, S> {
    /// Settles what `conclude` would have, touching nothing that could
    /// panic (no directory lookups): the last resort for an unwinding drop.
    fn settle_only(&mut self) {
        self.armed = false;
        let now = self.driver.now();
        let member = &self.asked.message.author_id;
        let mut state = self.driver.state();
        if self.reserved {
            state.pilot.guard.settle(member, false, now);
        }
        if let Some(stamp) = self.spent_at {
            state.pilot.allowance.refund(member, stamp);
        }
    }

    /// Concludes an armed question as a refunded failure, logs it and takes
    /// the 👀 off.
    fn abort(&mut self) {
        let concluded = self.conclude(&ended(), Err("not posted".into()), false);
        self.driver.shared.answerer.observe(&ChatEvent::Cancelled {
            interaction_id: &self.row_id,
            reason: "aborted",
        });
        let driver = self.driver.clone();
        let (channel, message) = (self.asked.channel_id.clone(), self.asked.message.id.clone());
        self.driver.spawn(async move {
            let shared = &driver.shared;
            if let Some(concluded) = concluded {
                let mut row = concluded.interaction;
                row.error = Some(FAILED.to_owned());
                shared.answerer.record(row).await;
            }
            shared
                .surface
                .unreact(&channel, &message, SEEN_REACTION)
                .await;
        });
    }
}

impl<A: Answerer, S: Surface> Drop for Held<A, S> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if !std::thread::panicking() {
            self.abort();
            return;
        }
        // Unwinding: a second panic here (the lookup that just failed, run
        // again by `conclude`) would abort the process. Settle what cannot
        // panic now; conclude and log later, off the unwinding stack.
        if self.deferred {
            self.settle_only();
            let driver = self.driver.clone();
            let (channel, message) = (self.asked.channel_id.clone(), self.asked.message.id.clone());
            self.driver.spawn(async move {
                driver
                    .shared
                    .surface
                    .unreact(&channel, &message, SEEN_REACTION)
                    .await;
            });
            return;
        }
        self.armed = false;
        let mut later = Held {
            driver: self.driver.clone(),
            asked: self.asked.clone(),
            prepared: Arc::clone(&self.prepared),
            ctx: self.ctx.clone(),
            row_id: self.row_id.clone(),
            spent_at: self.spent_at,
            reserved: self.reserved,
            started: self.started,
            armed: true,
            deferred: true,
        };
        self.driver.spawn(async move { later.abort() });
    }
}

impl<A: Answerer, S: Surface> ChatDriver<A, S> {
    pub(super) async fn worker(self, mut job: Queued, mut cancelled: Arc<AtomicBool>) {
        let origin = job.asked.origin_id.clone();
        loop {
            // A panicking question concludes through its `Held` guard; the
            // channel is still handed over below.
            let _ = CatchPanic(Box::pin(self.run_one(&job, &cancelled))).await;
            let next = {
                let mut state = self.state();
                state.running.remove(&job.asked.message.id);
                self.hand_off(&mut state, &origin)
            };
            match next {
                Some((next, flag)) => (job, cancelled) = (next, flag),
                None => return,
            }
        }
    }

    /// Remove a question's queue position, after its add has landed.
    pub(super) async fn keycap_off(&self, queued: &Queued) {
        let Some(position) = queued.position else {
            return;
        };
        if let Some(mut reacted) = queued.reacted.clone() {
            // Err: the add's task is gone; nothing more to wait for.
            let _ = reacted.wait_for(|done| *done).await.map(drop);
        }
        let asked = &queued.asked;
        self.shared
            .surface
            .unreact(
                &asked.channel_id,
                &asked.message.id,
                position_reaction(position),
            )
            .await;
    }

    /// Move every waiter's keycap to its current FIFO position once others
    /// left the queue. Each move waits for the keycap it replaces to land,
    /// and removal of the new one ([`Self::keycap_off`]) waits for its add.
    pub(super) fn renumber(&self, state: &mut State) {
        let State { pilot, waiting, .. } = state;
        for (id, queued) in waiting.iter_mut() {
            let (Some(old), Some(new)) = (queued.position, pilot.traffic.position(id)) else {
                continue;
            };
            queued.position = Some(new);
            let (from, to) = (position_reaction(old), position_reaction(new));
            if from == to {
                continue;
            }
            let (reacted, done) = watch::channel(false);
            let previous = queued.reacted.replace(done);
            let (channel, message) = (queued.asked.channel_id.clone(), id.clone());
            let driver = self.clone();
            self.spawn(async move {
                if let Some(mut previous) = previous {
                    // Err: the add's task is gone; nothing more to wait for.
                    let _ = previous.wait_for(|done| *done).await.map(drop);
                }
                let surface = &driver.shared.surface;
                surface.unreact(&channel, &message, from).await;
                surface.react(&channel, &message, to).await;
                reacted.send_replace(true);
            });
        }
    }

    /// Finish the channel's answer: give up stale waiters (refund, busy
    /// reaction), take the next waiting question, if any, and move the
    /// remaining waiters up.
    fn hand_off(&self, state: &mut State, origin: &str) -> Option<(Queued, Arc<AtomicBool>)> {
        let next = self.take_next(state, origin);
        self.renumber(state);
        next
    }

    fn take_next(&self, state: &mut State, origin: &str) -> Option<(Queued, Arc<AtomicBool>)> {
        let now = self.now();
        loop {
            let handoff = state.pilot.traffic.finish(origin, now);
            for expired in handoff.expired {
                if let Some(stamp) = expired.spent_at {
                    state.pilot.allowance.refund(&expired.member_id, stamp);
                }
                if let Some(queued) = state.waiting.remove(&expired.message_id) {
                    self.shared.answerer.observe(&ChatEvent::Cancelled {
                        interaction_id: &queued.row_id,
                        reason: "expired",
                    });
                    let driver = self.clone();
                    self.spawn(async move {
                        driver.keycap_off(&queued).await;
                        let asked = &queued.asked;
                        driver
                            .shared
                            .surface
                            .react(&asked.channel_id, &asked.message.id, CHANNEL_BUSY_REACTION)
                            .await;
                    });
                }
            }
            let next = handoff.next?;
            // `stop` empties the queue first; anything unknown is skipped.
            let Some(queued) = state.waiting.remove(&next.message_id) else {
                continue;
            };
            let cancelled = Arc::new(AtomicBool::new(false));
            state
                .running
                .insert(next.message_id.clone(), Arc::clone(&cancelled));
            return Some((queued, cancelled));
        }
    }

    /// Drop a question before any reservation or model work: refunded.
    fn refund(&self, job: &Queued) {
        if let Some(stamp) = job.spent_at {
            self.state()
                .pilot
                .allowance
                .refund(&job.asked.message.author_id, stamp);
        }
    }

    fn cancelled(&self, job: &Queued, reason: &'static str) {
        self.shared.answerer.observe(&ChatEvent::Cancelled {
            interaction_id: &job.row_id,
            reason,
        });
    }

    /// Chat is still on and this channel still in a chat category (both may
    /// have changed while the question waited). The channel is read afresh:
    /// the admission snapshot misses an ordinary channel's category move. A
    /// channel the directory no longer knows keeps its snapshot.
    fn still_admitted(&self, asked: &Asked) -> bool {
        let answerer = &self.shared.answerer;
        let setup = answerer.setup();
        let channels = answerer.channels();
        let snapshot = asked.gate.channel.as_ref();
        let fresh = snapshot.and_then(|channel| channels.channel(&channel.id));
        setup.enabled
            && setup.ready
            && setup.pilot.configured()
            && is_chat_channel(fresh.as_ref().or(snapshot), channels, &setup.pilot)
    }

    async fn run_one(&self, job: &Queued, cancelled: &Arc<AtomicBool>) {
        let shared = &self.shared;
        let (asked, surface) = (&job.asked, &shared.surface);
        let (channel, message_id) = (asked.channel_id.as_str(), asked.message.id.as_str());
        if !self.still_admitted(asked) {
            self.cancelled(job, "not_admitted");
            self.refund(job);
            self.keycap_off(job).await;
            return;
        }
        self.keycap_off(job).await;
        surface.react(channel, message_id, SEEN_REACTION).await;
        let started = Instant::now();
        let prepared = shared.answerer.prepare(asked).await;
        // Deleted while it was being prepared: no model call.
        let Some(prepared) = prepared.filter(|_| !cancelled.load(Ordering::SeqCst)) else {
            let reason = if cancelled.load(Ordering::SeqCst) {
                "deleted"
            } else {
                "not_ready"
            };
            self.cancelled(job, reason);
            self.refund(job);
            surface.unreact(channel, message_id, SEEN_REACTION).await;
            return;
        };
        let prepared = Arc::new(prepared);

        let row_id = job.row_id.clone();
        let ctx = tool_context(asked, &prepared, &row_id);
        // From here every path, a panic or an abort included, concludes
        // (refunding the allowance). The reservation is taken last in the
        // same locked block, recorded on `held` at once, so nothing can
        // unwind between reserving and `held` knowing to settle it.
        let mut held = Held {
            driver: self.clone(),
            asked: asked.clone(),
            ctx,
            prepared: Arc::clone(&prepared),
            row_id,
            spent_at: job.spent_at,
            reserved: false,
            started,
            armed: true,
            deferred: false,
        };
        let (reserved, turns, focus) = {
            let mut state = self.state();
            let now = self.now();
            if state.persona_key.as_deref() != Some(prepared.persona_key.as_str()) {
                if state.persona_key.is_some() {
                    state.pilot.conversations.forget(None);
                }
                state.persona_key = Some(prepared.persona_key.clone());
            }
            let pilot = &mut state.pilot;
            let focus = pilot.conversations.focus(&asked.origin_id, now);
            let turns = build_turns(
                &mut pilot.conversations,
                &asked.message,
                &asked.origin_id,
                now,
                asked.bot_user_id.as_deref().unwrap_or_default(),
                &*prepared.directory,
            );
            held.reserved = pilot.reserve_clean_retry(&asked.message.author_id, now);
            (held.reserved, turns, focus)
        };
        let config = &shared.config;
        let system = system_prompt(
            &prepared.persona,
            prepared.now,
            prepared.zone,
            prepared.reset,
            &prepared.model,
            &focus,
        );
        let question = Question {
            ctx: &held.ctx,
            conversation: assemble(&turns, system, config.model_context_tokens),
            reminder: prepared.persona.voice_reminder(),
            offer: ChatPilot::route(&asked.message.content, None, held.ctx.read_only),
            settings: AnswerSettings {
                tool_rounds: config.tool_rounds,
                timeout: config.timeout,
                reasoning: prepared.reasoning,
                temperature: None,
                max_output_tokens: config.max_output_tokens,
                model_context_tokens: config.model_context_tokens,
                clean_retry: reserved,
            },
        };
        let answered = shared.answerer.answer(Job {
            prepared: &prepared,
            asked,
            question,
            cancelled,
        });
        let (generation, was_cut) = tokio::select! {
            generation = answered => (generation, false),
            () = cut_signal(shared.cut.subscribe()) => (ended(), true),
        };
        let deleted = cancelled.load(Ordering::SeqCst);

        let posted = if deleted || was_cut {
            Err("not posted".to_owned())
        } else {
            let text = if generation.reply.is_empty() {
                failure_reply(&generation, &prepared.persona).to_owned()
            } else {
                generation.reply.clone()
            };
            surface.reply(channel, message_id, &text).await
        };
        let concluded = held.conclude(&generation, posted, deleted);
        // The row first: at shutdown the reaction tidy-up may be aborted.
        if let Some(concluded) = &concluded {
            let mut row = concluded.interaction.clone();
            if deleted {
                row.error = Some(DELETED.to_owned());
            } else if was_cut {
                row.error = Some(CUT.to_owned());
            }
            let event = if deleted || was_cut {
                ChatEvent::Cancelled {
                    interaction_id: &row.id,
                    reason: if deleted { "deleted" } else { "shutdown" },
                }
            } else {
                ChatEvent::Finished {
                    interaction: &row,
                    generation: &generation,
                    persona: prepared.persona.provenance(),
                    model: &prepared.model,
                    reasoning: prepared.reasoning,
                }
            };
            shared.answerer.observe(&event);
            shared.answerer.record(row).await;
            if let Some(alert) = &concluded.alert {
                shared.answerer.storm(alert);
            }
        }
        surface.unreact(channel, message_id, SEEN_REACTION).await;
    }
}
