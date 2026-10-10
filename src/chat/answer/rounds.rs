//! The tool loop over one governed question session (v4 `_loop`/`_chat`).
//!
//! Each round offers the tools snapshotted at its start (a `request_tools`
//! result applies from the next round), the last round and the round after a
//! posted card offer none, and every request is trimmed to the model context
//! first. A malformed, empty or content-filtered answer spends the session's
//! reserved clean retry: system prompt, the
//! asker's message and the voice reminder only.

use serde_json::Value;
use tokio::time::{Instant, timeout_at};

use super::cover::{grounding_listing, missing_reads};
use super::finish::{POSTED_UNFINISHED, finish};
use super::{
    AnswerFailure, CARD_NOT_POSTED, CONTEXT_BUDGET_REPLY, ChatPorts, Generation, GuildView,
    ModelRound, ProfanityGuard, ProfanityHit, ProfanitySide, Question, RoundOutcome,
};
use crate::chat::context::{budgeted, card_focus};
use crate::chat::tools::bundles::{Mode, ToolOffer};
use crate::chat::tools::dispatch;
use crate::chat::tools::propose::{ProposalCard, Proposer};
use crate::chat::tools::read::resolve::Heard;
use crate::chat::tools::read::{PendingCard, ToolWorld};
use crate::chat::tools::{FAILED, LOOKUP_FAILED, REFUSED, ToolName, ToolOutcome};
use crate::domain::drafts::ProposalStore;
use crate::domain::members::member_name;
use crate::domain::pytext::strip;
use crate::domain::schedule::ScheduleSnapshot;
use crate::domain::scheduler::{Clock, IdSource, ScheduleStore, Scope};
use crate::infrastructure::llm::governor::{SentRequest, Session, SessionError, SessionFailure};
use crate::infrastructure::llm::identity::PassthroughSession;
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

fn definition(offer: &ToolOffer, tool: ToolName) -> ToolDefinition {
    let schema = offer.schema(tool);
    let function = &schema["function"];
    ToolDefinition {
        name: tool.as_str().to_owned(),
        description: function["description"].as_str().map(str::to_owned),
        input_schema: function["parameters"].clone(),
    }
}

/// Copy the conversation without rewriting identity-bearing content.
fn passthrough(identity: &mut PassthroughSession, message: &Message) -> Message {
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

/// The asker's words in the conversation (a member turn reads `Name: text`):
/// the closing question, and their own message before a bot reply the
/// question follows directly. A closing turn that is not theirs (a rejection
/// follow-up's prompt) hears nothing.
fn heard<'c>(conversation: &'c [Message], speaker: &str) -> Heard<'c> {
    let said = |message: &'c Message| match message {
        Message::User { content } => content.strip_prefix(speaker)?.strip_prefix(": "),
        _ => None,
    };
    let mut before = conversation.iter().rev();
    let Some(question) = before.next().and_then(said) else {
        return Heard::default();
    };
    let earlier = match before.next() {
        Some(Message::Assistant { .. }) => before
            .find(|message| matches!(message, Message::User { .. }))
            .and_then(said),
        _ => None,
    };
    Heard { question, earlier }
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

fn tool_world<'a>(
    guild: &'a GuildView<'_>,
    snapshot: &'a ScheduleSnapshot,
    pending: &'a [PendingCard],
    heard: Heard<'a>,
) -> ToolWorld<'a> {
    ToolWorld {
        snapshot,
        members: guild.members,
        directory: guild.directory,
        catalog: guild.catalog,
        channels: guild.channels,
        pilot: guild.pilot,
        zone: guild.zone,
        reset_weekday: guild.reset_weekday,
        reset_time: guild.reset_time,
        pending,
        guides: guild.guides,
        run_ends: guild.run_ends,
        heard,
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
    /// `D-MIXED-PEOPLE`: the listing every finish grounds the reply against.
    grounding: Option<ToolOutcome>,
}

impl Loop<'_, '_> {
    fn request(
        &self,
        alias: &str,
        messages: Vec<Message>,
        offer: &ToolOffer,
        offered: &[ToolName],
    ) -> ChatRequest {
        let settings = &self.question.settings;
        ChatRequest {
            model: alias.to_owned(),
            messages,
            tools: offered
                .iter()
                .copied()
                .map(|tool| definition(offer, tool))
                .collect(),
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
        (bundles, latency_ms, clean, sent, estimate, request_ids): (
            Vec<String>,
            u64,
            bool,
            Option<SentRequest>,
            usize,
            Vec<String>,
        ),
    ) {
        if let Some(usage) = &response.usage {
            self.generation
                .add_usage(usage.prompt_tokens, usage.completion_tokens);
        }
        self.generation.model_rounds.push(ModelRound {
            round,
            reasoning_content: response
                .reasoning_content
                .as_deref()
                .and_then(crate::domain::model_log::capped_reasoning),
            reasoning_tokens: response.reasoning_tokens,
            content: response.content.clone(),
            requested_tools: response.tool_calls.iter().map(|c| c.name.clone()).collect(),
            finish_reason: Some(finish_name(&response.finish_reason)),
            bundles,
            latency_ms,
            clean,
            sent,
            prompt_tokens: response
                .usage
                .as_ref()
                .map(|usage| u64::from(usage.prompt_tokens)),
            completion_tokens: response
                .usage
                .as_ref()
                .map(|usage| u64::from(usage.completion_tokens)),
            prompt_estimate: u64::try_from(estimate).ok(),
            request_ids,
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
/// member-facing scrubbing, bounds). `alias` is the prepared route's alias;
/// a live switch before admission cannot redirect this session.
pub async fn run_question<P, S, I, C, X>(
    question: Question<'_>,
    alias: &str,
    session: &mut Session<'_, P>,
    identity: &mut PassthroughSession,
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
    let alias = alias.to_owned();
    let mut offer = question.offer.clone();
    let mut messages: Vec<Message> = question
        .conversation
        .iter()
        .map(|message| passthrough(identity, message))
        .collect();
    let speaker = member_name(guild.directory, &question.ctx.author_id);
    let heard = heard(&question.conversation, &speaker);
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
        grounding: None,
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
    // D-MIXED-PEOPLE coverage is checked once, at the first answer in words;
    // after code reads the next round is an answer in words, no tools.
    let mut coverage_checked = false;
    let mut answer_again = false;

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

    // D-MIXED-PEOPLE: read whoever the question names that no read covers
    // yet; `true` when reads were added for the model.
    macro_rules! cover {
        ($rounds:lifetime) => {{
            coverage_checked = true;
            let started = Instant::now();
            let loaded = within_deadline!($rounds, proposer.service.store().load(&Scope::All));
            // Nothing to read with: the reply stands as written.
            let mut added = false;
            if let Ok(snapshot) = loaded {
                // `get_schedule` reads no pending cards.
                let world = tool_world(guild, &snapshot, &[], heard);
                let outcomes = &mut state.generation.outcomes;
                let reads = missing_reads(question.ctx, &world, outcomes, round);
                added = !reads.is_empty();
                if added {
                    messages.push(Message::Assistant {
                        content: None,
                        tool_calls: reads
                            .iter()
                            .map(|(id, outcome)| ToolCallRequest {
                                id: id.clone(),
                                name: outcome.name.clone(),
                                arguments: Value::Object(outcome.arguments.clone()).to_string(),
                            })
                            .collect(),
                    });
                }
                let took_ms = millis(started);
                state.generation.tools_ms += took_ms;
                for (id, outcome) in reads {
                    messages.push(Message::Tool {
                        tool_call_id: id,
                        content: identity.tool_result(&outcome.output),
                    });
                    outcomes.push(RoundOutcome {
                        round,
                        outcome,
                        posted: Vec::new(),
                        took_ms,
                    });
                }
                state.grounding = grounding_listing(question.ctx, &world, outcomes);
            }
            added
        }};
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
        let with_tools = !last && !posted_write && !answer_again;
        let offered = if with_tools {
            offer.tools()
        } else {
            Vec::new()
        };
        // The last round answers without tools: whoever is still unread is
        // read first, so its answer covers everyone.
        if last && !coverage_checked && !question.ctx.schedule_people.is_empty() {
            cover!('rounds);
        }
        let outgoing = match budgeted(
            &mut messages,
            &offer.surface_text(),
            &state.reminder,
            context_tokens,
            settings.max_output_tokens as usize,
        ) {
            Ok(fits) => fits,
            Err(error) => {
                state.generation.failure = Some(AnswerFailure::ContextBudget(error));
                // "Shorten it and try again" would invite a duplicate of a
                // card this question already posted.
                state.generation.reply = if posted_write || !state.generation.posted.is_empty() {
                    POSTED_UNFINISHED
                } else {
                    CONTEXT_BUDGET_REPLY
                }
                .to_owned();
                break None;
            }
        };
        let request = state.request(&alias, outgoing.messages, &offer, &offered);
        let started = Instant::now();
        let first_id = session.request_ids().len();
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
        let sent = session.last_sent().cloned();
        let ids = session.request_ids()[first_id..].to_vec();
        state.record(
            round,
            &response,
            (bundles, latency, false, sent, outgoing.estimate, ids),
        );
        if response.tool_calls.is_empty() {
            let content = strip(response.content.as_deref().unwrap_or_default());
            if !content.is_empty() {
                // The draft left someone out: the model answers again over
                // every read, in words (a words answer in the last round was
                // covered before it was asked for).
                if !coverage_checked
                    && !question.ctx.schedule_people.is_empty()
                    && cover!('rounds)
                    && round < limit
                {
                    answer_again = true;
                    continue;
                }
                state.generation.reply = content.to_owned();
                break None;
            }
            break Some(Retry::Malformed);
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
                    let world = tool_world(guild, &snapshot, &pending, heard);
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
                    if !dispatched.superseded.is_empty() {
                        // Retirement already committed: finish its card refresh
                        // even when dispatch used the rest of the deadline.
                        ports.refresh_proposals(&dispatched.superseded).await;
                    }
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
                    (outcome, LOOKUP_FAILED.to_owned(), None)
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
                let took_ms = millis(started);
                state.generation.tools_ms += took_ms;
                state.generation.outcomes.push(RoundOutcome {
                    round,
                    outcome,
                    posted: Vec::new(),
                    took_ms,
                });
                state.generation.failure = Some(AnswerFailure::Timeout { seconds });
                break 'rounds None;
            }
            let mut posted = Vec::new();
            let mut undelivered = false;
            for card in &outcome.cards {
                let Ok(result) = timeout_at(deadline, ports.post_card(card)).await else {
                    // Log the call and what it created; this card's post is unknown.
                    let took_ms = millis(started);
                    state.generation.tools_ms += took_ms;
                    state.generation.posted.extend(posted.iter().cloned());
                    state.generation.outcomes.push(RoundOutcome {
                        round,
                        outcome,
                        posted,
                        took_ms,
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
                content = CARD_NOT_POSTED.to_owned();
            }
            let took_ms = millis(started);
            state.generation.tools_ms += took_ms;
            state.generation.posted.extend(posted.iter().cloned());
            messages.push(Message::Tool {
                tool_call_id: call.id.clone(),
                content,
            });
            state.generation.outcomes.push(RoundOutcome {
                round,
                outcome,
                posted,
                took_ms,
            });
        }
        // A posted write withholds tools from every later round, so a bundle
        // requested beside it is never offered: don't charge for it.
        if requested_now && !state.generation.posted.is_empty() {
            charged -= 1;
        }
    };

    let filtered = matches!(retry, Some(Retry::ContentBlocked));
    let params = (
        alias.as_str(),
        seconds,
        context_tokens,
        settings.max_output_tokens as usize,
        round,
    );
    let mut clean_base = Some(clean_base);
    match retry {
        Some(retry) if settings.clean_retry => {
            if let Some(base) = clean_base.take() {
                clean_retry(&mut state, retry, base, session, params).await;
            }
        }
        // Guarded off (per-member limit or storm guard): no retry is sent.
        Some(retry) => state.generation.failure = Some(retry.failure()),
        None => {}
    }
    // The model's own words, before finishing adds grounded records and
    // card lines built from store data (member names are not its words).
    let said = state.generation.reply.clone();
    let replaced = finish(&mut state.generation, state.grounding.as_ref());
    // An unposted write's fixed status text replaced the model's words
    // whole: members never see them, so there is nothing to check.
    if let Some(guard) = question.profanity.filter(|_| !replaced) {
        // The reserved clean retry, when this question still holds it.
        let base = clean_base.filter(|_| settings.clean_retry);
        profanity_check(&mut state, guard, &said, base, session, params).await;
    }
    let mut generation = state.generation;
    generation.requests = session.requests_used();
    // Only a session that sent a tagged request is findable in the gateway log.
    generation.session_id = (!session.request_ids().is_empty()).then(|| session.id().to_owned());
    session
        .request_ids()
        .clone_into(&mut generation.request_ids);
    generation.blocked = generation.reply.is_empty()
        && (filtered || generation.failure == Some(AnswerFailure::ContentBlocked));
    generation
}

/// The model-written reply text (`said`, before finishing) is checked; data
/// lines finishing adds are not. A hit spends the question's reserved clean
/// retry once, and a retry that is unavailable, fails or hits again sends
/// the deflection line.
async fn profanity_check<P: LlmProvider>(
    state: &mut Loop<'_, '_>,
    guard: &ProfanityGuard,
    said: &str,
    base: Option<Vec<Message>>,
    session: &mut Session<'_, P>,
    params: (&str, u64, usize, usize, u32),
) {
    let Some(word) = guard.reply_hit(said) else {
        return;
    };
    let mut sent = Some(guard.line().to_owned());
    if let Some(base) = base
        && let Ok(content) = send_clean(state, base, session, params).await
    {
        let clean = guard.reply_hit(&content).is_none();
        state.generation.reply = content;
        let _ = finish(&mut state.generation, state.grounding.as_ref());
        if clean && !state.generation.reply.is_empty() {
            sent = None;
        }
    }
    if let Some(line) = &sent {
        line.clone_into(&mut state.generation.reply);
    }
    state.generation.profanity = Some(ProfanityHit {
        side: ProfanitySide::Reply,
        word,
        sent,
    });
}

async fn clean_retry<P: LlmProvider>(
    state: &mut Loop<'_, '_>,
    retry: Retry,
    base: Vec<Message>,
    session: &mut Session<'_, P>,
    params: (&str, u64, usize, usize, u32),
) {
    let seconds = params.1;
    match send_clean(state, base, session, params).await {
        Ok(content) => state.generation.reply = content,
        Err(None) => state.generation.failure = Some(retry.failure()),
        Err(Some(error)) => {
            state.generation.failure = Some(match triage(error, seconds) {
                Ok(again) => again.failure(),
                Err(failure) => failure,
            });
        }
    }
}

/// Send the reserved clean retry: system prompt, the asker's message and the
/// voice reminder, no tools. `Ok` is its text answer; `Err(Some)` a session
/// error after a request went out, `Err(None)` no request or no text.
async fn send_clean<P: LlmProvider>(
    state: &mut Loop<'_, '_>,
    mut base: Vec<Message>,
    session: &mut Session<'_, P>,
    (alias, _seconds, context_tokens, reserve, round): (&str, u64, usize, usize, u32),
) -> Result<String, Option<SessionError>> {
    let outgoing =
        budgeted(&mut base, "[]", &state.reminder, context_tokens, reserve).map_err(|_| None)?;
    let request = state.request(alias, outgoing.messages, &state.question.offer, &[]);
    let started = Instant::now();
    let before = session.requests_used();
    let first_id = session.request_ids().len();
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
            return Err((!unsent).then_some(error));
        }
    };
    state.generation.clean_retry = true;
    let sent = session.last_sent().cloned();
    let ids = session.request_ids()[first_id..].to_vec();
    state.record(
        round,
        &response,
        (Vec::new(), latency, true, sent, outgoing.estimate, ids),
    );
    let content = strip(response.content.as_deref().unwrap_or_default());
    if response.tool_calls.is_empty() && !content.is_empty() {
        Ok(content.to_owned())
    } else {
        Err(None)
    }
}
