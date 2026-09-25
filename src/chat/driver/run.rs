//! One channel's worker: answer the question, then hand over to the next
//! waiting one until the channel's queue is empty.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};

use tokio::sync::watch;
use tokio::time::Instant;

use super::{Answerer, Asked, ChatDriver, Job, Prepared, Queued, State, Surface, new_row_id};
use crate::chat::answer::{AnswerFailure, AnswerSettings, Generation, Question};
use crate::chat::context::{assemble, build_turns, system_prompt};
use crate::chat::gate::{CHANNEL_BUSY_REACTION, SEEN_REACTION};
use crate::chat::pilot::{ChatPilot, Finished, LogFacts, ReplyPort, failure_reply};
use crate::chat::sanitize::schedule_defaults;
use crate::chat::tools::ToolContext;
use crate::infrastructure::llm::governor::{Charge, SessionError, SessionFailure};

use super::position_reaction;

const DELETED: &str = "cancelled: the question was deleted";
const CUT: &str = "cancelled: serve shut down";

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
/// state lock is held; a lock guard must not live across an await.
fn at_once<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(output) => output,
        Poll::Pending => unreachable!("conclude awaits only the already-posted reply"),
    }
}

/// A question dropped at shutdown: refunded, since its answer never came.
fn cut_short() -> Generation {
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
    ctx.bot_names.clone_from(&prepared.bot_names);
    let defaults = schedule_defaults(&asked.message.content, asked.bot_user_id.as_deref(), None);
    ctx.force_all_channels = defaults.force_all_channels;
    ctx.force_channel_scope = defaults.force_channel_scope;
    ctx.force_group_schedule = defaults.force_group_schedule;
    ctx.upcoming_only = defaults.upcoming_only;
    ctx
}

impl<A: Answerer, S: Surface> ChatDriver<A, S> {
    pub(super) async fn worker(self, mut job: Queued, mut cancelled: Arc<AtomicBool>) {
        let origin = job.asked.origin_id.clone();
        loop {
            self.run_one(&job, &cancelled).await;
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
                        let (asked, surface) = (&queued.asked, &driver.shared.surface);
                        if let Some(position) = queued.position {
                            let emoji = position_reaction(position);
                            surface
                                .unreact(&asked.channel_id, &asked.message.id, emoji)
                                .await;
                        }
                        surface
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

    async fn run_one(&self, job: &Queued, cancelled: &Arc<AtomicBool>) {
        let shared = &self.shared;
        let (asked, surface) = (&job.asked, &shared.surface);
        let (channel, message_id) = (asked.channel_id.as_str(), asked.message.id.as_str());
        if let Some(position) = job.position {
            surface
                .unreact(channel, message_id, position_reaction(position))
                .await;
        }
        surface.react(channel, message_id, SEEN_REACTION).await;
        let started = Instant::now();
        let Some(prepared) = shared.answerer.prepare(asked).await else {
            // Before any reservation or model work: a refunded no-op.
            if let Some(stamp) = job.spent_at {
                self.state()
                    .pilot
                    .allowance
                    .refund(&asked.message.author_id, stamp);
            }
            surface.unreact(channel, message_id, SEEN_REACTION).await;
            return;
        };

        // From here the reservation is held: every path reaches `conclude`.
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
        let config = &shared.config;
        let row_id = new_row_id();
        let ctx = tool_context(asked, &prepared, &row_id);
        let system = system_prompt(
            &prepared.persona,
            prepared.now,
            prepared.zone,
            prepared.reset,
            &prepared.model,
            &focus,
        );
        let question = Question {
            ctx: &ctx,
            conversation: assemble(&turns, system, config.model_context_tokens),
            reminder: prepared.persona.voice_reminder(),
            offer: ChatPilot::route(&asked.message.content, None, ctx.read_only),
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
            () = cut_signal(shared.cut.subscribe()) => (cut_short(), true),
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
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let concluded = {
            let mut state = self.state();
            let now = self.now();
            let done = Finished {
                message: &asked.message,
                channel_id: &asked.origin_id,
                ctx: &ctx,
                generation: &generation,
                persona: &prepared.persona,
                directory: &*prepared.directory,
                log: LogFacts {
                    id: row_id,
                    at: prepared.now,
                    model: &prepared.model,
                    reasoning: prepared.reasoning,
                    latency_ms,
                },
                spent_at: job.spent_at,
                reserved,
                now,
            };
            let concluded = at_once(state.pilot.conclude(done, &Posted(posted)));
            if deleted {
                // A deleted question is never quoted back to the model.
                state.pilot.conversations.withhold(message_id);
            }
            concluded
        };
        surface.unreact(channel, message_id, SEEN_REACTION).await;
        let mut row = concluded.interaction;
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
}
