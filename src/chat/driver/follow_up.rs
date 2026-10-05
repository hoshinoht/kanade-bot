//! Read-only clarification after the asker rejects a chatbot proposal card.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::delivery::{Cause, Deletion, Delivery, DeliveryRecord, Placeholder, append_error, lock};
use super::{Answerer, Asked, ChatDriver, Job, Prepared, Surface, new_row_id};
use crate::chat::answer::{
    AnswerFailure, AnswerSettings, Generation, Question, interaction, with_persona,
};
use crate::chat::context::{ChatTurn, TurnRole, assemble, system_prompt};
use crate::chat::gate::{IncomingMessage, is_chat_channel};
use crate::chat::pilot::{ChatPilot, failure_reply};
use crate::chat::sanitize::reply_parts;
use crate::chat::tools::ToolContext;
use crate::domain::members::member_name;
use crate::domain::model_log::ChatInteraction;
use crate::infrastructure::llm::governor::{Charge, SessionError, SessionFailure};

/// One rejected proposal's server-side card facts. They never come from a
/// Discord message, so a reactor cannot steer the prompt by editing text.
#[derive(Clone, Debug)]
pub struct FollowUpCard {
    pub summary: Option<String>,
    pub bosses: Vec<String>,
    pub participants: Vec<String>,
}

/// The reaction worker's safe hand-off after it has proved every rejected
/// proposal came from chat.
#[derive(Clone, Debug)]
pub struct FollowUpRequest {
    pub card_message_id: String,
    pub channel_id: String,
    pub reactor_id: String,
    pub source_ids: Vec<String>,
    pub cards: Vec<FollowUpCard>,
}

/// The reaction worker can ask the driver for a follow-up without knowing its
/// model, in-memory cooldown or shutdown bookkeeping.
pub trait RejectionFollowUp: Send + Sync {
    fn rejected(&self, request: FollowUpRequest) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

const COOLDOWN_S: f64 = 30.0;

fn fallback(card: &FollowUpCard) -> String {
    let bosses = card.bosses.join(" + ");
    match bosses.is_empty() {
        true => "the proposed change".to_owned(),
        false => format!("the proposed change for {bosses}"),
    }
}

fn prompt(request: &FollowUpRequest, prepared: &Prepared) -> String {
    let who = member_name(&*prepared.directory, &request.reactor_id);
    let facts = request
        .cards
        .iter()
        .map(|card| {
            let said = card
                .summary
                .as_deref()
                .filter(|summary| !summary.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| fallback(card));
            let party: Vec<String> = card
                .participants
                .iter()
                .map(|id| member_name(&*prepared.directory, id))
                .collect();
            if party.is_empty() {
                said
            } else {
                format!("{said} — for {}", party.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "[Note from the scheduler, not from anybody in the channel.] {who} just reacted ❌ on the card you posted for them, so it is off and nothing has changed. The card said: {facts}. Write one short message to {who}: say the card is off, and ask what they would like instead -- the day, the time, the boss, whoever is on it, whichever of those you cannot tell from the card. Ask, do not guess, and do not apologise at length. You cannot post a card in this message; their reply comes back to you as a normal message and you can post the corrected card then."
    )
}

fn ended() -> Generation {
    Generation::failed(AnswerFailure::Session(SessionError {
        failure: SessionFailure::Ended,
        charge: Charge::Refunded,
    }))
}

async fn cut_signal(mut cut: tokio::sync::watch::Receiver<bool>) {
    while !*cut.borrow_and_update() {
        if cut.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// A follow-up is not a normal admitted question, but it still has to settle
/// its in-memory channel slot and log a shared delivery record when its task
/// panics or shutdown aborts it.
struct FollowUpHeld<A: Answerer, S: Surface> {
    driver: ChatDriver<A, S>,
    request: FollowUpRequest,
    origin_id: String,
    record: Arc<Mutex<DeliveryRecord>>,
    prepared: Option<Arc<Prepared>>,
    ctx: Option<ToolContext>,
    prompt: Option<String>,
    generation: Option<Generation>,
    armed: bool,
    deferred: bool,
}

fn row(
    prepared: &Prepared,
    ctx: &ToolContext,
    prompt: &str,
    generation: &Generation,
    record: &DeliveryRecord,
    cause: Option<Cause>,
) -> ChatInteraction {
    let mut row = interaction(
        new_row_id(),
        prepared.now,
        ctx,
        prompt,
        generation,
        &prepared.model,
        prepared.reasoning,
        0,
    );
    row.reply = if generation.reply.is_empty() {
        failure_reply(generation, &prepared.persona).to_owned()
    } else {
        generation.reply.clone()
    };
    with_persona(&mut row, prepared.persona.provenance());
    if let Some(guardrail) = row.guardrail.as_object_mut() {
        guardrail.insert("kind".into(), json!("rejection_followup"));
        guardrail.insert(
            "context".into(),
            json!({
                "window": prepared.context_window,
                "reserve": prepared.max_output_tokens,
                "source": prepared.context_source,
            }),
        );
        guardrail.insert("delivery".into(), record.guardrail());
    }
    match cause {
        Some(Cause::Shutdown) => row.error = Some("cancelled: serve shut down".into()),
        Some(Cause::Aborted) if record.landed.is_empty() => {
            row.error = Some("failed: the question stopped unexpectedly".into());
        }
        _ => {}
    }
    if let Some(incomplete) = record.incomplete_error() {
        append_error(&mut row.error, incomplete);
    }
    row
}

impl<A: Answerer, S: Surface> FollowUpHeld<A, S> {
    fn release_only(&mut self) {
        self.armed = false;
        self.driver
            .state()
            .rejection_followups
            .remove(&self.origin_id);
    }

    fn remember(&self, reply: String, id: String) {
        let assistant = ChatTurn::new(TurnRole::Assistant, reply, Some(id.clone()));
        let mut state = self.driver.state();
        state
            .pilot
            .conversations
            .remember(&self.origin_id, assistant.clone(), self.driver.now());
        state
            .pilot
            .conversations
            .anchor_assistant(&id, &self.origin_id, assistant);
    }

    /// `kept_out`: the reply was replaced by the profanity line, which never
    /// enters context, not even through a member's reply to it.
    async fn complete(&mut self, row: ChatInteraction, reply: String, kept_out: bool) {
        let record = lock(&self.record).clone();
        if kept_out {
            exclude_landed(&mut self.driver.state().pilot.conversations, &record);
        } else if let Some(id) = record.first_id() {
            self.remember(reply, id);
        }
        self.driver.shared.answerer.record(row).await;
        self.release_only();
    }

    fn abort(&mut self) {
        let cause = if *self.driver.shared.cut.borrow() {
            Cause::Shutdown
        } else {
            Cause::Aborted
        };
        let failure_edit = {
            let mut record = lock(&self.record);
            let failure_edit = match &record.placeholder {
                Placeholder::Live(id) if record.in_flight.is_none() && !record.answer_started => {
                    Some(id.clone())
                }
                _ => None,
            };
            record.aborted(cause);
            failure_edit
        };
        let Some(prepared) = self.prepared.clone() else {
            self.release_only();
            return;
        };
        let Some(ctx) = self.ctx.clone() else {
            self.release_only();
            return;
        };
        let prompt = self.prompt.clone().unwrap_or_default();
        let generation = self.generation.clone().unwrap_or_else(ended);
        let reply = if generation.reply.is_empty() {
            failure_reply(&generation, &prepared.persona).to_owned()
        } else {
            generation.reply.clone()
        };
        self.armed = false;
        let driver = self.driver.clone();
        let request = self.request.clone();
        let origin_id = self.origin_id.clone();
        let record_handle = Arc::clone(&self.record);
        self.driver.spawn(async move {
            if let Some(id) = failure_edit {
                let outcome = driver
                    .shared
                    .surface
                    .edit(&request.channel_id, &id, &reply)
                    .await;
                lock(&record_handle).placeholder = match outcome {
                    super::Effect::Done(()) => Placeholder::Edited,
                    super::Effect::UnknownMessage => Placeholder::Deleted,
                    _ => Placeholder::Orphaned,
                };
            }
            let record = lock(&record_handle).clone();
            let row = row(&prepared, &ctx, &prompt, &generation, &record, Some(cause));
            if generation.kept_out_of_context() {
                exclude_landed(&mut driver.state().pilot.conversations, &record);
            } else if let Some(id) = record.first_id() {
                let assistant = ChatTurn::new(TurnRole::Assistant, reply.clone(), Some(id.clone()));
                let mut state = driver.state();
                state
                    .pilot
                    .conversations
                    .remember(&origin_id, assistant.clone(), driver.now());
                state
                    .pilot
                    .conversations
                    .anchor_assistant(&id, &origin_id, assistant);
            }
            driver.shared.answerer.record(row).await;
            driver.state().rejection_followups.remove(&origin_id);
        });
    }
}

/// Every landed part of a replaced follow-up reply leaves context for good.
fn exclude_landed(
    conversations: &mut crate::chat::context::Conversations,
    record: &DeliveryRecord,
) {
    for landed in &record.landed {
        conversations.exclude(&landed.id);
    }
}

impl<A: Answerer, S: Surface> Drop for FollowUpHeld<A, S> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if !std::thread::panicking() {
            self.abort();
            return;
        }
        // Never redo prompt construction or a directory lookup while another
        // panic unwinds. Defer the complete record, with slot release as the
        // panic-free fallback if that task itself panics.
        if self.deferred {
            self.release_only();
            return;
        }
        self.armed = false;
        let mut later = Self {
            driver: self.driver.clone(),
            request: self.request.clone(),
            origin_id: self.origin_id.clone(),
            record: Arc::clone(&self.record),
            prepared: self.prepared.clone(),
            ctx: self.ctx.clone(),
            prompt: self.prompt.clone(),
            generation: self.generation.take(),
            armed: true,
            deferred: true,
        };
        self.driver.spawn(async move { later.abort() });
    }
}

impl<A: Answerer, S: Surface> RejectionFollowUp for ChatDriver<A, S> {
    fn rejected(&self, request: FollowUpRequest) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.offer_follow_up(request).await;
        })
    }
}

impl<A: Answerer, S: Surface> ChatDriver<A, S> {
    async fn offer_follow_up(&self, request: FollowUpRequest) {
        if request.source_ids.is_empty() || request.cards.is_empty() {
            return;
        }
        let setup = self.shared.answerer.setup();
        let channels = self.shared.answerer.channels();
        let channel = channels.channel(&request.channel_id);
        if !(setup.enabled
            && setup.ready
            && setup.pilot.configured()
            && is_chat_channel(channel.as_ref(), channels, &setup.pilot))
        {
            return;
        }
        if !self.shared.answerer.owns_rejection(&request).await {
            return;
        }

        // The source load above can wait; make both live checks again before
        // reserving the channel's in-memory cooldown.
        let setup = self.shared.answerer.setup();
        let channels = self.shared.answerer.channels();
        let channel = channels.channel(&request.channel_id);
        if !(setup.enabled
            && setup.ready
            && setup.pilot.configured()
            && is_chat_channel(channel.as_ref(), channels, &setup.pilot))
        {
            return;
        }
        let origin_id = channel
            .as_ref()
            .and_then(|channel| channel.parent_id.clone())
            .unwrap_or_else(|| request.channel_id.clone());
        let now = self.now();
        {
            let mut state = self.state();
            if state.closed
                || state.rejection_followups.contains(&origin_id)
                || state.pilot.traffic.view().answering.contains(&origin_id)
                || state
                    .pilot
                    .traffic
                    .view()
                    .waiting
                    .iter()
                    .any(|waiting| waiting.channel_id == origin_id)
                || state
                    .followed_up_at
                    .get(&origin_id)
                    .is_some_and(|then| now - then < COOLDOWN_S)
            {
                return;
            }
            state.followed_up_at.insert(origin_id.clone(), now);
            state.rejection_followups.insert(origin_id.clone());
        }
        let driver = self.clone();
        self.spawn(async move { driver.run_follow_up(request, origin_id).await });
    }

    async fn run_follow_up(&self, request: FollowUpRequest, origin_id: String) {
        let record = Arc::new(Mutex::<DeliveryRecord>::default());
        let mut held = FollowUpHeld {
            driver: self.clone(),
            request: request.clone(),
            origin_id: origin_id.clone(),
            record: Arc::clone(&record),
            prepared: None,
            ctx: None,
            prompt: None,
            generation: None,
            armed: true,
            deferred: false,
        };
        let asked = Asked {
            message: crate::chat::context::QuestionMessage {
                id: request.card_message_id.clone(),
                author_id: request.reactor_id.clone(),
                content: String::new(),
                reference: None,
            },
            channel_id: request.channel_id.clone(),
            origin_id: origin_id.clone(),
            gate: IncomingMessage::default(),
            replied_author_id: None,
            bot_user_id: None,
            self_role_id: None,
            is_admin: false,
        };
        let Some(prepared) = self.shared.answerer.prepare(&asked).await else {
            held.release_only();
            return;
        };
        let prepared = Arc::new(prepared);
        held.prepared = Some(Arc::clone(&prepared));
        let prompt = prompt(&request, &prepared);
        let mut ctx = ToolContext::new(
            request.reactor_id.clone(),
            request.channel_id.clone(),
            request.card_message_id.clone(),
            prepared.now,
        );
        ctx.read_only = true;
        ctx.bot_names.clone_from(&prepared.bot_names);
        held.ctx = Some(ctx.clone());
        held.prompt = Some(prompt.clone());
        let now = self.now();
        let (turns, focus) = {
            let mut state = self.state();
            let focus = state.pilot.conversations.focus(&origin_id, now);
            let mut turns = state.pilot.conversations.history(&origin_id, now);
            // This is prompt-only. It must never be remembered as a hidden
            // channel note after the visible assistant reply lands.
            turns.push(ChatTurn::new(TurnRole::User, prompt.clone(), None));
            (turns, focus)
        };
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
            conversation: assemble(
                &turns,
                system,
                prepared.context_window,
                prepared.max_output_tokens as usize,
            ),
            reminder: prepared.persona.voice_reminder(),
            offer: ChatPilot::route(&prompt, None, true),
            settings: AnswerSettings {
                tool_rounds: self.shared.config.tool_rounds,
                timeout: self.shared.config.timeout,
                reasoning: prepared.reasoning,
                temperature: None,
                max_output_tokens: prepared.max_output_tokens,
                model_context_tokens: prepared.context_window,
                clean_retry: false,
            },
            profanity: Some(&prepared.profanity),
        };
        let deletion = Deletion::default();
        let delivery = Delivery::new(
            &self.shared.surface,
            &request.channel_id,
            &request.card_message_id,
            Arc::clone(&record),
            &deletion,
        );
        let answer = self.shared.answerer.answer(Job {
            prepared: &prepared,
            asked: &asked,
            question,
            cancelled: &deletion.flag,
        });
        let answer = delivery
            .until_answered(
                prepared.persona.staging_lines().generic.clone(),
                answer,
                cut_signal(self.shared.cut.subscribe()),
            )
            .await;
        let was_cut = answer.is_none();
        let generation = answer.unwrap_or_else(ended);
        if !was_cut {
            held.generation = Some(generation.clone());
        }
        let reply = if generation.reply.is_empty() {
            failure_reply(&generation, &prepared.persona).to_owned()
        } else {
            generation.reply.clone()
        };
        if was_cut {
            if matches!(
                lock(&record).placeholder,
                super::delivery::Placeholder::Live(_)
            ) {
                delivery.deliver(vec![reply.clone()]).await;
            }
            delivery.ended_early(Cause::Shutdown).await;
        } else {
            delivery.deliver(reply_parts(&reply)).await;
        }
        let delivery_record = lock(&record).clone();
        let row = row(
            &prepared,
            &ctx,
            &prompt,
            &generation,
            &delivery_record,
            was_cut.then_some(Cause::Shutdown),
        );
        let kept_out = generation.kept_out_of_context();
        held.complete(row, reply, kept_out).await;
    }
}
