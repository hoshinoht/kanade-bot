//! One channel's worker: answer the question, then hand over to the next
//! waiting one until the channel's queue is empty. A question that holds
//! the clean-retry reservation always concludes, even when its future is
//! dropped (a panic or a shutdown abort): see [`Held`].

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use tokio::sync::watch;
use tokio::time::Instant;

use super::{Answerer, Asked, ChatDriver, Job, Prepared, Queued, State, Surface, new_row_id};
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
    ctx.upcoming_only = defaults.upcoming_only;
    ctx
}

/// A question holding its clean-retry reservation. Concluding it disarms
/// it; dropped armed (panic, abort) it concludes as a refunded failure, so
/// the reservation is settled and nothing is posted.
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
        self.armed = false;
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

impl<A: Answerer, S: Surface> Drop for Held<A, S> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let concluded = self.conclude(&ended(), Err("not posted".into()), false);
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

    /// Finish the channel's answer: give up stale waiters (refund, busy
    /// reaction) and take the next waiting question, if any.
    fn hand_off(&self, state: &mut State, origin: &str) -> Option<(Queued, Arc<AtomicBool>)> {
        let now = self.now();
        loop {
            let handoff = state.pilot.traffic.finish(origin, now);
            for expired in handoff.expired {
                if let Some(stamp) = expired.spent_at {
                    state.pilot.allowance.refund(&expired.member_id, stamp);
                }
                if let Some(queued) = state.waiting.remove(&expired.message_id) {
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

    /// Chat is still on and this channel still in a chat category (both may
    /// have changed while the question waited).
    fn still_admitted(&self, asked: &Asked) -> bool {
        let answerer = &self.shared.answerer;
        let setup = answerer.setup();
        setup.enabled
            && setup.ready
            && setup.pilot.configured()
            && is_chat_channel(
                asked.gate.channel.as_ref(),
                answerer.channels(),
                &setup.pilot,
            )
    }

    async fn run_one(&self, job: &Queued, cancelled: &Arc<AtomicBool>) {
        let shared = &self.shared;
        let (asked, surface) = (&job.asked, &shared.surface);
        let (channel, message_id) = (asked.channel_id.as_str(), asked.message.id.as_str());
        if !self.still_admitted(asked) {
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
            self.refund(job);
            surface.unreact(channel, message_id, SEEN_REACTION).await;
            return;
        };
        let prepared = Arc::new(prepared);

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
            let reserved = pilot.reserve_clean_retry(&asked.message.author_id, now);
            let focus = pilot.conversations.focus(&asked.origin_id, now);
            let turns = build_turns(
                &mut pilot.conversations,
                &asked.message,
                &asked.origin_id,
                now,
                asked.bot_user_id.as_deref().unwrap_or_default(),
                &*prepared.directory,
            );
            (reserved, turns, focus)
        };
        let row_id = new_row_id();
        // From here every path, a panic or an abort included, concludes.
        let mut held = Held {
            driver: self.clone(),
            asked: asked.clone(),
            ctx: tool_context(asked, &prepared, &row_id),
            prepared: Arc::clone(&prepared),
            row_id,
            spent_at: job.spent_at,
            reserved,
            started,
            armed: true,
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
            shared.answerer.record(row).await;
            if let Some(alert) = &concluded.alert {
                shared.answerer.storm(alert);
            }
        }
        surface.unreact(channel, message_id, SEEN_REACTION).await;
    }
}
