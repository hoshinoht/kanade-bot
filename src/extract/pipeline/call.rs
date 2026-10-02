//! One governed extraction call (the `tests/extract/session.rs` reference
//! loop): build a raw prompt, `complete` then at most one `answer_retry` in the
//! same extraction session, then `plan_burst`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::time::Instant;

use super::config::CallContext;
use super::extractor::{CALL_SWITCHED_OFF, Extractor, utc};
use super::ports::{Outbox, Proposer};
use crate::domain::catalog::BossTable;
use crate::domain::model_log::{ModelLogStore, ReadMessage, WatchedMessage};
use crate::domain::schedule::{FixedRun, Run, ScheduleSnapshot};
use crate::domain::scheduler::ScheduleStore;
use crate::domain::weeks;
use crate::extract::Amendment;
use crate::extract::AmendmentKind;
use crate::extract::plan::{BurstInputs, BurstMessage, Payload, Planned, plan_burst};
use crate::extract::prompt::{
    PromptContext, PromptMessage, build_messages, estimate_messages,
    extraction_request_with_reserve, member_name, prompt_text,
};
use crate::extract::resolve::Resolved;
use crate::extract::schema::{AttemptOutcome, ExtractionAttempts, ExtractionCall, Next};
use crate::infrastructure::llm::governor::{Refused, RoleRoute, SessionError, SessionFailure};
use crate::infrastructure::llm::identity::{Member, PassthroughSession};
use crate::infrastructure::llm::{Effort, ErrorCode, LlmProvider, Message, Usage};

/// Why a call produced no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The governor or the gateway turned it away; nothing ran upstream.
    TurnedAway {
        retry_at: Option<Instant>,
    },
    /// The provider's content filter stopped the answer (`ContentFiltered`).
    ContentBlocked,
    Failed,
}

/// Token usage over one call's attempts (`complete` and at most one
/// `answer_retry`). The reported pair sums the attempts whose reply carried
/// usage, and the estimate covers those same attempts; when none reported,
/// the estimate covers every attempt that was sent and the pair stays unset.
/// An attempt is sent when the session's request count moved during it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CallUsage {
    /// Prompt, completion and estimate over the attempts that reported usage.
    reported: Option<(u64, u64, u64)>,
    /// Estimate over every attempt that was sent.
    sent: Option<u64>,
}

impl CallUsage {
    fn attempt(&mut self, sent: bool, estimate: u64, usage: Option<&Usage>) {
        if let Some(usage) = usage {
            let (prompt, completion, reported) = self.reported.get_or_insert((0, 0, 0));
            *prompt = prompt.saturating_add(u64::from(usage.prompt_tokens));
            *completion = completion.saturating_add(u64::from(usage.completion_tokens));
            *reported = reported.saturating_add(estimate);
        }
        if sent || usage.is_some() {
            let total = self.sent.get_or_insert(0);
            *total = total.saturating_add(estimate);
        }
    }

    pub fn prompt_tokens(&self) -> Option<u64> {
        self.reported.map(|(prompt, _, _)| prompt)
    }

    pub fn completion_tokens(&self) -> Option<u64> {
        self.reported.map(|(_, completion, _)| completion)
    }

    pub fn prompt_estimate(&self) -> Option<u64> {
        self.reported.map(|(_, _, estimate)| estimate).or(self.sent)
    }
}

/// What prompts are built from, loaded once per burst.
pub(super) struct Loaded {
    pub snapshot: ScheduleSnapshot,
    /// The channel's cached messages from 48 h before the burst on.
    pub history: Vec<WatchedMessage>,
    pub members: Vec<Member>,
    pub bosses: Arc<BossTable>,
    pub channel_name: String,
}

pub(super) struct Prepared<'a> {
    pub messages: Vec<Message>,
    pub anchor: DateTime<Utc>,
    pub channel_runs: Vec<&'a Run>,
    pub guild_runs: Vec<&'a Run>,
    pub burst_order: Vec<String>,
    pub author_ids: HashMap<String, String>,
    pub message_times: HashMap<String, DateTime<Utc>>,
    pub burst_messages: Vec<BurstMessage>,
}

/// A planned change that outlives the snapshot it was matched against.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Kept {
    pub amendment: Amendment,
    pub resolved: Resolved,
    pub run: Option<Run>,
    pub payload: Payload,
    pub match_reason: String,
    pub match_code: &'static str,
    pub also_mentioned: Vec<AmendmentKind>,
    pub ambiguous: bool,
    pub summary: String,
}

impl Kept {
    pub fn from_planned(planned: Planned<'_>) -> Self {
        Self {
            amendment: planned.amendment,
            resolved: planned.resolved,
            run: planned.run.cloned(),
            payload: planned.payload,
            match_reason: planned.match_reason,
            match_code: planned.match_code,
            also_mentioned: planned.also_mentioned,
            ambiguous: planned.ambiguous,
            summary: planned.summary,
        }
    }

    pub fn planned(&self) -> Planned<'_> {
        Planned {
            amendment: self.amendment.clone(),
            resolved: self.resolved,
            run: self.run.as_ref(),
            payload: self.payload.clone(),
            match_reason: self.match_reason.clone(),
            match_code: self.match_code,
            also_mentioned: self.also_mentioned.clone(),
            ambiguous: self.ambiguous,
            summary: self.summary.clone(),
        }
    }
}

/// One model call and, after commit, what came of it: one extraction log row.
#[derive(Clone, Debug)]
pub(crate) struct CallRecord {
    pub log_id: String,
    pub at: DateTime<Utc>,
    pub model: String,
    /// The level sent: the live route's, else the pipeline's configured one.
    pub reasoning: Option<Effort>,
    pub prompt: String,
    pub raw: String,
    pub latency_ms: Option<u64>,
    pub requests: u32,
    pub message_ids: Vec<String>,
    pub member_ids: Vec<String>,
    /// Message id -> author id, context included.
    pub authors: HashMap<String, String>,
    pub burst: Vec<crate::extract::backlog::BacklogEntry>,
    /// The burst as read, for the conditional processed mark.
    pub read: Vec<ReadMessage>,
    pub error: Option<String>,
    pub failure: Option<Failure>,
    pub kept: Vec<Kept>,
    pub dropped: usize,
    pub stale: usize,
    pub proposal_ids: Vec<String>,
    pub refusals: Vec<crate::domain::model_log::ExtractionRefusal>,
    pub redirected: usize,
    /// How each lead-in of this call was made (`LineSource::as_str`), for the log.
    pub nudges: Vec<&'static str>,
    /// Sent to an external route with raw member data.
    pub external_unmasked: bool,
    /// Its messages were edited or deleted before the commit: nothing kept.
    pub stale_version: bool,
    pub context_window: usize,
    pub context_reserve: usize,
    pub context_source: &'static str,
    pub usage: CallUsage,
}

impl CallRecord {
    pub fn fail(&mut self, failure: Failure, error: String) {
        self.failure = Some(failure);
        self.error = Some(error);
    }

    pub fn ok(&self) -> bool {
        self.failure.is_none()
    }
}

fn author_name(members: &[Member], user_id: &str) -> String {
    members
        .iter()
        .find(|member| member.user_id == user_id)
        .map(|member| member_name(member).to_owned())
        .unwrap_or_else(|| {
            // v4 `_name_for`: the last four characters of an unknown id.
            let tail: String = {
                let chars: Vec<char> = user_id.chars().collect();
                chars[chars.len().saturating_sub(4)..].iter().collect()
            };
            format!("user{tail}")
        })
}

fn prompt_message(row: &WatchedMessage, members: &[Member]) -> PromptMessage {
    PromptMessage {
        id: row.id.clone(),
        author_id: row.author_id.clone(),
        author_name: author_name(members, &row.author_id),
        created_at: row.created_at,
        content: row.content.clone(),
    }
}

/// How a session error is reported to `ExtractionAttempts`, and what the
/// log calls it.
fn classify(error: &SessionError, limit: Duration) -> (AttemptOutcome, Failure) {
    let failed = || AttemptOutcome::Failed {
        detail: error.to_string(),
    };
    match &error.failure {
        SessionFailure::Refused(refused) => match refused {
            Refused::Unavailable { retry_at } => (
                failed(),
                Failure::TurnedAway {
                    retry_at: *retry_at,
                },
            ),
            Refused::RateLimited { wait } => (
                failed(),
                Failure::TurnedAway {
                    retry_at: Some(Instant::now() + *wait),
                },
            ),
            Refused::Timeout | Refused::Busy => (failed(), Failure::TurnedAway { retry_at: None }),
            // Configuration refusals never clear by waiting, and an exhausted
            // retry budget must not be worked around by requeueing.
            Refused::UnknownRole
            | Refused::Ungrouped
            | Refused::ExternalForbidden
            | Refused::MustNotWait
            | Refused::RetryBudgetExhausted => (failed(), Failure::Failed),
        },
        SessionFailure::Model(model) => match model.code {
            // Nothing ran upstream: read the burst again once the gateway
            // or the (now open) breaker lets it through.
            ErrorCode::AdmissionRefused | ErrorCode::BackendUnavailable => {
                (failed(), Failure::TurnedAway { retry_at: None })
            }
            ErrorCode::DeadlineExceeded | ErrorCode::UpstreamTimeout => {
                (AttemptOutcome::TimedOut { limit }, Failure::Failed)
            }
            ErrorCode::ContentFiltered => (failed(), Failure::ContentBlocked),
            ErrorCode::UnsupportedCapability
            | ErrorCode::ProviderAuthentication
            | ErrorCode::ModelMismatch => (
                AttemptOutcome::Misconfigured {
                    detail: error.to_string(),
                },
                Failure::Failed,
            ),
            _ => (failed(), Failure::Failed),
        },
        _ => (failed(), Failure::Failed),
    }
}

impl<S, P, X, O> Extractor<S, P, X, O>
where
    S: ScheduleStore + ModelLogStore + Send + Sync,
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
{
    /// An empty record for `rows`, before any call.
    pub(super) fn record(
        &self,
        rows: &[WatchedMessage],
        route: Option<&RoleRoute>,
        context: CallContext,
    ) -> CallRecord {
        let mut members: Vec<String> = rows.iter().map(|row| row.author_id.clone()).collect();
        members.sort();
        members.dedup();
        let mut message_ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
        message_ids.sort();
        CallRecord {
            log_id: self.new_id(),
            at: self.clock.now(),
            reasoning: route
                .and_then(|route| route.effort)
                .or(self.config.reasoning),
            model: route.map(|route| route.alias.clone()).unwrap_or_default(),
            prompt: String::new(),
            raw: String::new(),
            latency_ms: None,
            requests: 0,
            message_ids,
            member_ids: members,
            authors: rows
                .iter()
                .map(|row| (row.id.clone(), row.author_id.clone()))
                .collect(),
            burst: rows.iter().map(super::extractor::entry).collect(),
            read: rows.iter().map(super::extractor::read_of).collect(),
            error: None,
            failure: None,
            kept: Vec::new(),
            dropped: 0,
            stale: 0,
            proposal_ids: Vec::new(),
            refusals: Vec::new(),
            redirected: 0,
            nudges: Vec::new(),
            external_unmasked: false,
            stale_version: false,
            context_window: context.window,
            context_reserve: context.reserve,
            context_source: context.source,
            usage: CallUsage::default(),
        }
    }

    /// v4 `_prepare`: the burst, the messages just before it, this and next
    /// boss week's runs, the channel's timings and the roster.
    pub(super) fn prepare<'a>(
        &self,
        channel_id: &str,
        loaded: &'a Loaded,
        chunk: &[WatchedMessage],
        session: &mut PassthroughSession,
    ) -> Prepared<'a> {
        let zone = self.config.zone;
        let members = &loaded.members;
        let burst: Vec<PromptMessage> = chunk
            .iter()
            .map(|row| prompt_message(row, members))
            .collect();
        let first = chunk.first().map(|row| row.created_at).unwrap_or_default();
        let anchor = chunk.last().map(|row| row.created_at).unwrap_or_default();
        let ids: HashSet<&str> = chunk.iter().map(|row| row.id.as_str()).collect();
        // Anchored to the burst, never the wall clock: a rescan replays old
        // conversations and must not see their answers as context.
        let before: Vec<&WatchedMessage> = loaded
            .history
            .iter()
            .filter(|row| {
                row.created_at >= first - super::config::CONTEXT_WINDOW
                    && row.created_at < first
                    && !ids.contains(row.id.as_str())
            })
            .collect();
        let limit = self.config.context_messages;
        let context: Vec<PromptMessage> = before[before.len().saturating_sub(limit)..]
            .iter()
            .map(|row| prompt_message(row, members))
            .collect();

        let weeks: Vec<DateTime<Utc>> = weeks::week_start(
            &anchor,
            zone,
            self.config.reset_weekday,
            self.config.reset_time,
        )
        .ok()
        .into_iter()
        .flat_map(|this| {
            let next = weeks::week_end(&this, zone).ok();
            std::iter::once(utc(&this)).chain(next.as_ref().map(utc))
        })
        .collect();
        let guild_runs: Vec<&Run> = loaded
            .snapshot
            .runs
            .iter()
            .filter(|run| weeks.contains(&run.week_start))
            .collect();
        let channel_runs: Vec<&Run> = guild_runs
            .iter()
            .copied()
            .filter(|run| run.channel_id.as_deref() == Some(channel_id))
            .collect();
        let fixed_runs: Vec<FixedRun> = loaded
            .snapshot
            .fixed_runs
            .iter()
            .filter(|fixed| fixed.channel_id.as_deref() == Some(channel_id))
            .cloned()
            .collect();
        let context_obj = PromptContext {
            zone,
            table: &loaded.bosses,
            burst: &burst,
            context: &context,
            runs: &channel_runs,
            fixed_runs: &fixed_runs,
            roster: members,
            channel_name: &loaded.channel_name,
            guild_runs: &guild_runs,
        };
        let messages = build_messages(&context_obj, session);
        let ordered = context.iter().chain(&burst);
        Prepared {
            messages,
            anchor,
            burst_order: ordered.clone().map(|m| m.id.clone()).collect(),
            author_ids: ordered
                .clone()
                .map(|m| (m.id.clone(), m.author_id.clone()))
                .collect(),
            message_times: ordered.map(|m| (m.id.clone(), m.created_at)).collect(),
            burst_messages: chunk
                .iter()
                .map(|row| BurstMessage {
                    id: row.id.clone(),
                    author_id: row.author_id.clone(),
                    content: row.content.clone(),
                })
                .collect(),
            channel_runs,
            guild_runs,
        }
    }

    /// One governed call over `chunk`, planned; never fails, the record
    /// says what happened.
    pub(super) async fn call(
        &self,
        channel_id: &str,
        loaded: &Loaded,
        chunk: &[WatchedMessage],
        route: Option<&RoleRoute>,
        context: CallContext,
    ) -> CallRecord {
        let mut record = self.record(chunk, route, context);
        // The pass's route: the alias its context was resolved for, so a
        // mid-pass model switch never pairs one model with another's window.
        let Some(route) = route else {
            record.fail(
                Failure::Failed,
                "the extraction model is not configured".into(),
            );
            return record;
        };
        // Alias and level are pinned for the pass; the session keeps them.
        record.model.clone_from(&route.alias);
        record.reasoning = route.effort.or(self.config.reasoning);
        let mut identity = PassthroughSession;
        let prepared = self.prepare(channel_id, loaded, chunk, &mut identity);
        record.prompt = prompt_text(&prepared.messages);
        record.authors.extend(prepared.author_ids.clone());

        let timeout = self.config.call_timeout;
        let mark = self.cut_mark();
        // No request once extraction is off.
        if !self.guild.extraction_enabled() {
            record.fail(Failure::Failed, CALL_SWITCHED_OFF.into());
            return record;
        }
        let opened = tokio::select! {
            biased;
            cut = self.cut(mark) => Err(cut),
            opened = self
                .client
                .open_extraction_on(route, channel_id, self.config.permit_wait, timeout) => Ok(opened),
        };
        let opened = match opened {
            Ok(opened) => opened,
            Err(cut) => {
                record.fail(Failure::Failed, cut.into());
                return record;
            }
        };
        let mut session = match opened {
            Ok(session) => session,
            Err(error) => {
                let (_, failure) = classify(&error, timeout);
                record.fail(failure, error.to_string());
                return record;
            }
        };
        // The route's alias and group were pinned when the governed session opened.
        let alias = route.alias.clone();
        let started = Instant::now();
        let mut attempts = ExtractionAttempts::new(prepared.messages.clone());
        let mut failure = None;
        let mut first = true;
        let call: ExtractionCall = loop {
            let request = extraction_request_with_reserve(
                &alias,
                attempts.messages().to_vec(),
                record.reasoning,
                u32::try_from(context.reserve).unwrap_or(u32::MAX),
            );
            let estimate = u64::try_from(estimate_messages(&request.messages)).unwrap_or(u64::MAX);
            let before = session.requests_used();
            let sending = async {
                if first {
                    session.complete(&request).await
                } else {
                    session.answer_retry(&request).await
                }
            };
            let sent = tokio::select! {
                biased;
                cut = self.cut(mark) => Err(cut),
                sent = sending => Ok(sent),
            };
            let sent = match sent {
                Ok(sent) => sent,
                Err(cut) => {
                    record
                        .usage
                        .attempt(session.requests_used() > before, estimate, None);
                    record.model = alias;
                    record.requests = session.requests_used();
                    record.external_unmasked = route.external && record.requests > 0;
                    record.latency_ms =
                        Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
                    record.fail(Failure::Failed, cut.into());
                    return record;
                }
            };
            first = false;
            record.usage.attempt(
                session.requests_used() > before,
                estimate,
                sent.as_ref()
                    .ok()
                    .and_then(|response| response.usage.as_ref()),
            );
            let outcome = match sent {
                Ok(response) => AttemptOutcome::Reply {
                    content: response.content,
                    reasoning: None,
                },
                Err(error) => {
                    let (outcome, kind) = classify(&error, timeout);
                    failure = Some(kind);
                    outcome
                }
            };
            match attempts.record(outcome) {
                Next::Retry => continue,
                Next::Done(call) => break call,
            }
        };
        record.model = alias;
        record.requests = session.requests_used();
        record.external_unmasked = route.external && record.requests > 0;
        record.latency_ms = Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        record.raw = if call.raw.is_empty() {
            call.error.clone().unwrap_or_default()
        } else {
            call.raw.clone()
        };
        drop(session);

        let Some(extraction) = call.extraction else {
            record.fail(
                failure.unwrap_or(Failure::Failed),
                call.error.unwrap_or_else(|| "no answer".into()),
            );
            return record;
        };
        let inputs = BurstInputs {
            anchor: prepared.anchor,
            now: self.clock.now(),
            zone: self.config.zone,
            reset_weekday: self.config.reset_weekday,
            reset_time: self.config.reset_time,
            channel_runs: &prepared.channel_runs,
            guild_runs: &prepared.guild_runs,
            burst_order: &prepared.burst_order,
            author_ids: &prepared.author_ids,
            message_times: &prepared.message_times,
            min_confidence: self.config.min_confidence,
            boss_table: Some(&loaded.bosses),
            burst_messages: &prepared.burst_messages,
        };
        match plan_burst(&extraction, &inputs) {
            Ok(plan) => {
                record.dropped = plan.dropped.len();
                record.stale = plan
                    .dropped
                    .iter()
                    .filter(|entry| entry.match_reason == "already passed")
                    .count();
                record.kept = plan.planned.into_iter().map(Kept::from_planned).collect();
            }
            Err(error) => record.fail(Failure::Failed, error.to_string()),
        }
        record
    }
}
