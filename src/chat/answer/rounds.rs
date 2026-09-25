//! The tool loop over one governed question session (v4 `_loop`/`_chat`).
//!
//! Each round offers the tools snapshotted at its start (a `request_tools`
//! result applies from the next round), the last round and the round after a
//! posted card offer none, and every request is trimmed to the model context
//! first. A malformed, empty or undecodable answer, or a content-filtered
//! one, spends the session's reserved clean retry: system prompt, the
//! asker's message and the voice reminder only.

use serde_json::Value;
use tokio::time::{Instant, timeout_at};

use super::finish::finish;
use super::{
    AnswerFailure, CARD_NOT_POSTED, ChatPorts, Generation, GuildView, ModelRound, Question,
    RoundOutcome,
};
use crate::chat::context::{budgeted, card_focus};
use crate::chat::tools::bundles::{Mode, ToolOffer};
use crate::chat::tools::dispatch;
use crate::chat::tools::propose::{ProposalCard, Proposer};
use crate::chat::tools::read::ToolWorld;
use crate::chat::tools::schemas::surface_text;
use crate::chat::tools::{FAILED, LOOKUP_FAILED, REFUSED, ToolName, ToolOutcome};
use crate::domain::drafts::ProposalStore;
use crate::domain::members::member_name;
use crate::domain::pytext::strip;
use crate::domain::scheduler::{Clock, IdSource, ScheduleStore, Scope};
use crate::infrastructure::llm::governor::{Session, SessionError, SessionFailure};
use crate::infrastructure::llm::identity::IdentitySession;
use crate::infrastructure::llm::{
    ChatRequest, CompletionResponse, ErrorCode, FinishReason, LlmProvider, Message, Sampling,
    ToolCallRequest, ToolDefinition,
};

/// Why the reserved clean retry is spent.
#[derive(Clone, Copy)]
enum Retry {
    Malformed,
    ContentBlocked,
}

impl Retry {
    fn failure(self) -> AnswerFailure {
        match self {
            Self::Malformed => AnswerFailure::Malformed,
            Self::ContentBlocked => AnswerFailure::ContentBlocked,
        }
    }
}

fn millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn definition(tool: ToolName) -> ToolDefinition {
    let schema = tool.schema();
    let function = &schema["function"];
    ToolDefinition {
        name: tool.as_str().to_owned(),
        description: function["description"].as_str().map(str::to_owned),
        input_schema: function["parameters"].clone(),
    }
}

fn encode(identity: &mut dyn IdentitySession, message: &Message) -> Message {
    match message {
        Message::System { content } => Message::System {
            content: identity.text(content),
        },
        Message::User { content } => Message::User {
            content: identity.text(content),
        },
        Message::Assistant {
            content,
            tool_calls,
        } => Message::Assistant {
            content: content.as_deref().map(|text| identity.text(text)),
            tool_calls: tool_calls.clone(),
        },
        Message::Tool {
            tool_call_id,
            content,
        } => Message::Tool {
            tool_call_id: tool_call_id.clone(),
            content: identity.tool_result(content),
        },
    }
}

fn finish_name(reason: &FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop".into(),
        FinishReason::ToolCalls => "tool_calls".into(),
        FinishReason::Length => "length".into(),
        FinishReason::ContentFilter => "content_filter".into(),
        FinishReason::Other(other) => other.clone(),
    }
}

fn bundle_names(offer: &ToolOffer, offered: bool) -> Vec<String> {
    match (offered, offer.mode()) {
        (false, _) => Vec::new(),
        (true, Mode::FullSet) => vec!["full".into()],
        (true, Mode::Dynamic) => offer
            .bundles()
            .iter()
            .map(|bundle| bundle.request_name().unwrap_or("read").to_owned())
            .collect(),
    }
}

/// A session error the clean retry answers, or the failure it is.
fn triage(error: SessionError, seconds: u64) -> Result<Retry, AnswerFailure> {
    match &error.failure {
        SessionFailure::Model(model) => match model.code {
            ErrorCode::InvalidOutput => Ok(Retry::Malformed),
            ErrorCode::ContentFiltered => Ok(Retry::ContentBlocked),
            ErrorCode::DeadlineExceeded | ErrorCode::UpstreamTimeout => {
                Err(AnswerFailure::Timeout { seconds })
            }
            _ => Err(AnswerFailure::Session(error)),
        },
        SessionFailure::Ended => Err(AnswerFailure::Timeout { seconds }),
        _ => Err(AnswerFailure::Session(error)),
    }
}

struct Loop<'q, 'g> {
    question: &'q Question<'q>,
    guild: &'q GuildView<'g>,
    generation: Generation,
    reminder: String,
}

impl Loop<'_, '_> {
    fn request(&self, alias: &str, messages: Vec<Message>, offered: &[ToolName]) -> ChatRequest {
        let settings = &self.question.settings;
        ChatRequest {
            model: alias.to_owned(),
            messages,
            tools: offered.iter().copied().map(definition).collect(),
            output_schema: None,
            max_output_tokens: settings.max_output_tokens,
            reasoning: settings.reasoning,
            sampling: settings.temperature.map(|temperature| Sampling {
                temperature: Some(temperature),
                ..Sampling::default()
            }),
        }
    }

    fn record(
        &mut self,
        round: u32,
        response: &CompletionResponse,
        identity: &dyn IdentitySession,
        (bundles, latency_ms, clean): (Vec<String>, u64, bool),
    ) {
        if let Some(usage) = &response.usage {
            self.generation
                .add_usage(usage.prompt_tokens, usage.completion_tokens);
        }
        let content = response
            .content
            .as_ref()
            .map(|text| identity.decode_reply(text).unwrap_or_else(|_| text.clone()));
        self.generation.model_rounds.push(ModelRound {
            round,
            content,
            requested_tools: response.tool_calls.iter().map(|c| c.name.clone()).collect(),
            finish_reason: Some(finish_name(&response.finish_reason)),
            bundles,
            latency_ms,
            clean,
        });
    }

    fn focus(&self, card: &ProposalCard) -> String {
        let party: Vec<String> = card
            .participants
            .iter()
            .map(|uid| member_name(self.guild.directory, uid))
            .collect();
        card_focus(&card.summary, &party)
    }
}

/// Run one question over an open session; never fails, the generation says
/// what happened. The reply is finished (write/read claims, grounding,
/// member-facing scrubbing, bounds) and identity-decoded.
pub async fn run_question<P, S, I, C, X>(
    question: Question<'_>,
    session: &mut Session<'_, P>,
    identity: &mut dyn IdentitySession,
    guild: &GuildView<'_>,
    proposer: &mut Proposer<'_, S, I, C>,
    ports: &X,
) -> Generation
where
    P: LlmProvider,
    S: ScheduleStore + ProposalStore + Sync,
    I: IdSource,
    C: Clock,
    X: ChatPorts,
{
    let alias = session.alias().unwrap_or_default().to_owned();
    let mut offer = question.offer.clone();
    let mut messages: Vec<Message> = question
        .conversation
        .iter()
        .map(|message| encode(identity, message))
        .collect();
    let clean_base: Vec<Message> = messages
        .first()
        .into_iter()
        .chain(
            messages
                .iter()
                .rev()
                .find(|message| matches!(message, Message::User { .. })),
        )
        .cloned()
        .collect();
    let mut state = Loop {
        reminder: identity.text(&question.reminder),
        question: &question,
        guild,
        generation: Generation::default(),
    };
    let settings = question.settings;
    let seconds = settings.timeout.as_secs();
    let context_tokens = settings.model_context_tokens;
    let cap = u32::from(settings.tool_rounds);
    // The question's deadline bounds tool work and card posting too, as v4's
    // `wait_for` bounded the whole loop.
    let deadline = session.deadline();
    let mut charged = 0u32;
    let mut round = 0u32;

    // The loop label is passed in: macro hygiene hides one written here.
    macro_rules! within_deadline {
        ($rounds:lifetime, $work:expr) => {
            match timeout_at(deadline, $work).await {
                Ok(done) => done,
                Err(_) => {
                    state.generation.failure = Some(AnswerFailure::Timeout { seconds });
                    break $rounds None;
                }
            }
        };
    }

    let retry = 'rounds: loop {
        round += 1;
        let limit = cap.saturating_sub(charged);
        if round > limit {
            state.generation.failure = Some(AnswerFailure::KeptCallingTools);
            break None;
        }
        state.generation.rounds = round;
        let last = round >= limit;
        offer.begin_round();
        // A bundle requested now must leave a round that offers it and a
        // final no-tools round after the charge; otherwise refuse the request.
        if round + 3 > limit {
            offer.close_requests();
        }
        let posted_write = state.generation.outcomes.iter().any(|o| {
            !o.posted.is_empty() && ToolName::parse(&o.outcome.name).is_some_and(ToolName::is_write)
        });
        let with_tools = !last && !posted_write;
        let offered = if with_tools {
            offer.tools()
        } else {
            Vec::new()
        };
        let outgoing = match budgeted(
            &mut messages,
            &surface_text(&offered),
            &state.reminder,
            context_tokens,
        ) {
            Ok(outgoing) => outgoing,
            Err(error) => {
                state.generation.failure = Some(AnswerFailure::ContextBudget(error));
                break None;
            }
        };
        let request = state.request(&alias, outgoing, &offered);
        let started = Instant::now();
        let sent = session.complete(&request).await;
        let latency = millis(started);
        state.generation.model_ms += latency;
        let response = match sent {
            Ok(response) => response,
            Err(error) => match triage(error, seconds) {
                Ok(retry) => break Some(retry),
                Err(failure) => {
                    state.generation.failure = Some(failure);
                    break None;
                }
            },
        };
        let bundles = bundle_names(&offer, with_tools);
        state.record(round, &response, identity, (bundles, latency, false));
        if response.tool_calls.is_empty() {
            let content = strip(response.content.as_deref().unwrap_or_default());
            match identity.decode_reply(content) {
                Ok(reply) if !content.is_empty() => {
                    state.generation.reply = reply;
                    break None;
                }
                _ => break Some(Retry::Malformed),
            }
        }
        messages.push(Message::Assistant {
            content: Some(strip(response.content.as_deref().unwrap_or_default()).to_owned()),
            tool_calls: response
                .tool_calls
                .iter()
                .map(|call| ToolCallRequest {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                })
                .collect(),
        });
        let mut requested_now = false;
        for call in &response.tool_calls {
            state.generation.tool_calls.push(call.name.clone());
            let started = Instant::now();
            let loaded = within_deadline!('rounds, proposer.service.store().load(&Scope::All));
            let (mut outcome, mut content, requested) = match loaded {
                Ok(snapshot) => {
                    let pending = within_deadline!('rounds, ports.pending());
                    let world = ToolWorld {
                        snapshot: &snapshot,
                        members: guild.members,
                        directory: guild.directory,
                        catalog: guild.catalog,
                        channels: guild.channels,
                        pilot: guild.pilot,
                        zone: guild.zone,
                        reset_weekday: guild.reset_weekday,
                        reset_time: guild.reset_time,
                        pending: &pending,
                        guides: guild.guides,
                    };
                    let arguments = Value::String(call.arguments.clone());
                    // Never cancelled mid-flight: staging and supersede must
                    // finish together; the deadline is checked right after.
                    let dispatched = dispatch::run(
                        question.ctx,
                        &world,
                        &mut offer,
                        proposer,
                        identity,
                        &call.name,
                        &arguments,
                    )
                    .await;
                    (
                        dispatched.outcome,
                        dispatched.model_content,
                        dispatched.requested,
                    )
                }
                Err(error) => {
                    let outcome = ToolOutcome {
                        name: call.name.clone(),
                        output: LOOKUP_FAILED.to_owned(),
                        arguments: serde_json::Map::new(),
                        ok: false,
                        error: Some(FAILED),
                        created: Vec::new(),
                        cards: Vec::new(),
                        detail: Some(error.to_string()),
                    };
                    (outcome, identity.tool_result(LOOKUP_FAILED), None)
                }
            };
            if requested.is_some() {
                charged += 1;
                requested_now = true;
            }
            // Recorded before the deadline check and posting, so a timeout
            // still logs the proposal this call created.
            state
                .generation
                .created
                .extend(outcome.created.iter().cloned());
            if Instant::now() >= deadline {
                // Kept so the log's round shows the call; its cards stay unposted.
                state.generation.tools_ms += millis(started);
                state.generation.outcomes.push(RoundOutcome {
                    round,
                    outcome,
                    posted: Vec::new(),
                });
                state.generation.failure = Some(AnswerFailure::Timeout { seconds });
                break 'rounds None;
            }
            let mut posted = Vec::new();
            let mut undelivered = false;
            for card in &outcome.cards {
                let Ok(result) = timeout_at(deadline, ports.post_card(card)).await else {
                    // Log the call and what it created; this card's post is unknown.
                    state.generation.tools_ms += millis(started);
                    state.generation.posted.extend(posted.iter().cloned());
                    state.generation.outcomes.push(RoundOutcome {
                        round,
                        outcome,
                        posted,
                    });
                    state.generation.failure = Some(AnswerFailure::Timeout { seconds });
                    break 'rounds None;
                };
                match result {
                    Ok(()) => {
                        posted.push(card.proposal_id.clone());
                        state.generation.focus = Some(state.focus(card));
                    }
                    Err(_) => undelivered = true,
                }
            }
            if undelivered && posted.is_empty() {
                outcome.ok = false;
                outcome.error = Some(REFUSED);
                CARD_NOT_POSTED.clone_into(&mut outcome.output);
                content = identity.tool_result(CARD_NOT_POSTED);
            }
            state.generation.tools_ms += millis(started);
            state.generation.posted.extend(posted.iter().cloned());
            messages.push(Message::Tool {
                tool_call_id: call.id.clone(),
                content,
            });
            state.generation.outcomes.push(RoundOutcome {
                round,
                outcome,
                posted,
            });
        }
        // A posted write withholds tools from every later round, so a bundle
        // requested beside it is never offered: don't charge for it.
        if requested_now && !state.generation.posted.is_empty() {
            charged -= 1;
        }
    };

    match retry {
        Some(retry) if settings.clean_retry => {
            clean_retry(
                &mut state,
                retry,
                clean_base,
                session,
                identity,
                (&alias, seconds, context_tokens, round),
            )
            .await;
        }
        // Guarded off (per-member limit or storm guard): no retry is sent.
        Some(retry) => state.generation.failure = Some(retry.failure()),
        None => {}
    }
    let mut generation = state.generation;
    generation.requests = session.requests_used();
    finish(&mut generation);
    generation
}

async fn clean_retry<P: LlmProvider>(
    state: &mut Loop<'_, '_>,
    retry: Retry,
    mut base: Vec<Message>,
    session: &mut Session<'_, P>,
    identity: &mut dyn IdentitySession,
    (alias, seconds, context_tokens, round): (&str, u64, usize, u32),
) {
    let outgoing = match budgeted(&mut base, "[]", &state.reminder, context_tokens) {
        Ok(outgoing) => outgoing,
        Err(_) => {
            state.generation.failure = Some(retry.failure());
            return;
        }
    };
    let request = state.request(alias, outgoing, &[]);
    let started = Instant::now();
    let before = session.requests_used();
    let sent = session.clean_retry(&request).await;
    let latency = millis(started);
    state.generation.model_ms += latency;
    let response = match sent {
        Ok(response) => response,
        Err(error) => {
            // Counted, not inferred from the failure kind: a requeue that
            // loses its permit or an ended session may or may not have sent.
            let unsent = session.requests_used() == before;
            state.generation.clean_retry = !unsent;
            state.generation.failure = Some(if unsent {
                retry.failure()
            } else {
                match triage(error, seconds) {
                    Ok(again) => again.failure(),
                    Err(failure) => failure,
                }
            });
            return;
        }
    };
    state.generation.clean_retry = true;
    state.record(round, &response, identity, (Vec::new(), latency, true));
    let content = strip(response.content.as_deref().unwrap_or_default());
    match identity.decode_reply(content) {
        Ok(reply) if response.tool_calls.is_empty() && !content.is_empty() => {
            state.generation.reply = reply;
        }
        _ => state.generation.failure = Some(retry.failure()),
    }
}
