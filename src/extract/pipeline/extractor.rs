//! One burst end to end (v4 `Pipeline.flush` / `extract`): read the burst and
//! its context from the messages cache, gate it, cut it to the prompt budget,
//! call the model once per piece through a governed extraction session, plan,
//! propose, and write one extraction log row per call.

use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

use super::call::{CallRecord, Failure, Loaded};
use super::config::{CONTEXT_WINDOW, PipelineConfig, RECENT_SCHEDULING};
use super::ports::{Guild, IncomingMessage, Outbox, Proposer, SelfServiceDeps};
use crate::domain::model_log::{MessageUpsert, ModelLogStore, ReadMessage, WatchedMessage};
use crate::domain::scheduler::{Clock, IdSource, ScheduleStore, Scope, StoreError};
use crate::domain::weeks;
use crate::extract::backlog::BacklogEntry;
use crate::extract::gate::{self, BossLexicon, GateResult};
use crate::extract::prompt::{estimate_messages, prompt_budget, schema_instruction_tokens};
use crate::extract::schema::extraction_schema;
use crate::extract::window::split_until;
use crate::infrastructure::llm::LlmProvider;
use crate::infrastructure::llm::governor::ModelClient;
use crate::infrastructure::llm::identity::IdentityCodec;

/// Logged (and shown by the admin portal) when the schedule store fails;
/// store text can carry paths, so it goes only to the server log.
pub const SCHEDULE_UNREADABLE: &str = "the schedule could not be read";
pub const HISTORY_UNREADABLE: &str = "the channel history could not be read";
/// The error of a call cut by [`Extractor::cancel_calls`] (logged `failed`;
/// its messages stay unprocessed for a later read).
pub const CALL_CANCELLED: &str = "cancelled: serve shut down";

/// The fixed sentence for a call log; the store's own text goes to stderr
/// as a structured event.
fn store_failure(sentence: &'static str, error: &StoreError) -> String {
    let event = serde_json::json!({
        "level": "WARN",
        "event": "extraction_store_failed",
        "stage": sentence,
        "error": error.to_string(),
    });
    eprintln!("{event}");
    sentence.to_owned()
}

/// Everything an [`Extractor`] is built from.
pub struct Deps<S, P, X, O> {
    /// The schedule (reads only) and the model-log store.
    pub store: Arc<S>,
    pub client: Arc<ModelClient<P>>,
    pub codec: Arc<dyn IdentityCodec>,
    pub guild: Arc<dyn Guild>,
    pub proposer: Arc<X>,
    pub outbox: Arc<O>,
    pub clock: Arc<dyn Clock + Send + Sync>,
    /// Extraction log ids.
    pub ids: Box<dyn IdSource + Send>,
    /// Links and nudges; `None` keeps v4's cards-only behaviour.
    pub self_service: Option<SelfServiceDeps>,
}

/// What one pass (a burst, or a rescan channel) did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassReport {
    /// Extraction log ids written, one per model call.
    pub logs: Vec<String>,
    pub proposals: Vec<String>,
    /// Up-front refusals, as logged.
    pub refused: Vec<String>,
    pub redirected: usize,
    pub answers: usize,
    pub dropped: usize,
    /// Dropped as already passed.
    pub stale: usize,
    /// Calls that failed, and anything the pass could not write.
    pub errors: Vec<String>,
    /// Messages of calls the governor turned away, to be read again later.
    pub turned_away: Vec<BacklogEntry>,
    /// When the governor said to try again (breaker probe or rate wait).
    pub retry_at: Option<tokio::time::Instant>,
}

pub struct Extractor<S, P, X, O> {
    pub(super) store: Arc<S>,
    pub(super) client: Arc<ModelClient<P>>,
    pub(super) codec: Arc<dyn IdentityCodec>,
    pub(super) guild: Arc<dyn Guild>,
    pub(super) proposer: Arc<X>,
    pub(super) outbox: Arc<O>,
    pub(super) clock: Arc<dyn Clock + Send + Sync>,
    ids: Mutex<Box<dyn IdSource + Send>>,
    pub(super) self_service: Option<SelfServiceDeps>,
    pub(super) config: PipelineConfig,
    /// Set once at shutdown: calls and permit waits in flight end at once.
    cancel: tokio::sync::watch::Sender<bool>,
}

impl<S, P, X, O> std::fmt::Debug for Extractor<S, P, X, O> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Extractor")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// A gated message: roster member, keyword gate hit.
pub(super) fn gated(
    message: &WatchedMessage,
    guild: &dyn Guild,
    lexicon: &BossLexicon<'_>,
    roster: &[String],
) -> Option<GateResult> {
    if !guild.has_role(&message.author_id) {
        return None;
    }
    let result = gate::evaluate(&message.content, lexicon, roster);
    result.hit().then_some(result)
}

/// What a read saw; marking processed later is conditional on it, so an edit
/// arriving during a call is read again.
pub(super) fn read_of(message: &WatchedMessage) -> ReadMessage {
    ReadMessage {
        id: message.id.clone(),
        content: message.content.clone(),
    }
}

pub(super) fn entry(message: &WatchedMessage) -> BacklogEntry {
    BacklogEntry {
        channel_id: message.channel_id.clone(),
        message_id: message.id.clone(),
        created_at: message.created_at,
    }
}

impl<S, P, X, O> Extractor<S, P, X, O>
where
    S: ScheduleStore + ModelLogStore + Send + Sync,
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
{
    pub fn new(deps: Deps<S, P, X, O>, config: PipelineConfig) -> Self {
        Self {
            store: deps.store,
            client: deps.client,
            codec: deps.codec,
            guild: deps.guild,
            proposer: deps.proposer,
            outbox: deps.outbox,
            clock: deps.clock,
            ids: Mutex::new(deps.ids),
            self_service: deps.self_service,
            config,
            cancel: tokio::sync::watch::Sender::new(false),
        }
    }

    /// Cut every model call and permit wait in flight, and any started
    /// later: each is logged with [`CALL_CANCELLED`]. For a bounded stop.
    pub fn cancel_calls(&self) {
        self.cancel.send_replace(true);
    }

    /// Resolves once [`Self::cancel_calls`] was called.
    pub(crate) async fn cancelled(&self) {
        let mut cancel = self.cancel.subscribe();
        let _ = cancel.wait_for(|cancelled| *cancelled).await;
    }

    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    pub(crate) fn new_id(&self) -> String {
        self.ids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .new_id()
    }

    pub(super) fn roster_ids(&self) -> Vec<String> {
        self.guild
            .members()
            .into_iter()
            .map(|member| member.user_id)
            .collect()
    }

    /// Cache a member's message in a watched channel (v4 stores before any
    /// offer, whether or not extraction is on). `None`: not cached at all.
    pub async fn store_message(
        &self,
        message: &IncomingMessage,
    ) -> Result<Option<MessageUpsert>, StoreError> {
        if message.author != super::ports::AuthorKind::Member
            || !self.guild.is_watched(&message.channel_id)
        {
            return Ok(None);
        }
        let row = WatchedMessage {
            id: message.id.clone(),
            channel_id: message.channel_id.clone(),
            author_id: message.author_id.clone(),
            created_at: message.created_at,
            edited_at: message.edited_at,
            content: message.content.clone(),
            processed_at: None,
        };
        self.store.upsert_message(row).await.map(Some)
    }

    /// The gate result for a cached message, when it may join a burst.
    pub(super) fn gate_incoming(&self, message: &IncomingMessage) -> Option<GateResult> {
        if message.handled_by_chat
            || !self.guild.extraction_enabled()
            || !self.guild.has_role(&message.author_id)
        {
            return None;
        }
        let bosses = self.guild.bosses();
        let lexicon = BossLexicon::new(&bosses);
        let result = gate::evaluate(&message.content, &lexicon, &self.roster_ids());
        result.hit().then_some(result)
    }

    /// Read one buffered burst (live debounce or backlog): its unprocessed,
    /// gated messages, one extraction unless it is only answers in a quiet
    /// channel (then marked processed with no call).
    pub async fn flush(&self, channel_id: &str, burst: &[BacklogEntry]) -> PassReport {
        let mut report = PassReport::default();
        if burst.is_empty() || !self.guild.extraction_enabled() {
            return report;
        }
        let wanted: HashSet<&str> = burst.iter().map(|m| m.message_id.as_str()).collect();
        let since = burst.iter().map(|m| m.created_at).min().unwrap_or_default();
        let cached = match self.store.channel_messages(channel_id, since, false).await {
            Ok(rows) => rows,
            Err(error) => {
                report.errors.push(error.to_string());
                return report;
            }
        };
        let bosses = self.guild.bosses();
        let lexicon = BossLexicon::new(&bosses);
        let roster = self.roster_ids();
        let (rows, results): (Vec<WatchedMessage>, Vec<GateResult>) = cached
            .into_iter()
            .filter(|row| wanted.contains(row.id.as_str()) && row.processed_at.is_none())
            .filter_map(|row| {
                let result = gated(&row, self.guild.as_ref(), &lexicon, &roster)?;
                Some((row, result))
            })
            .unzip();
        if rows.is_empty() {
            return report;
        }
        let now = self.clock.now();
        let recent = match self
            .store
            .channel_messages(channel_id, now - RECENT_SCHEDULING, false)
            .await
        {
            Ok(rows) => rows,
            Err(error) => {
                report.errors.push(error.to_string());
                return report;
            }
        };
        let scheduling = recent
            .iter()
            .filter(|row| !wanted.contains(row.id.as_str()))
            .any(|row| gate::evaluate(&row.content, &lexicon, &roster).strong());
        if !gate::should_extract(&results, scheduling) {
            let read: Vec<ReadMessage> = rows.iter().map(read_of).collect();
            if let Err(error) = self.store.mark_read(&read, now).await {
                report.errors.push(error.to_string());
            }
            return report;
        }
        let records = self.call_burst(channel_id, rows).await;
        let consolidate = records.len() > 1;
        self.commit(channel_id, records, consolidate).await
    }

    /// Everything the prompts for `rows` are built from.
    async fn load(&self, channel_id: &str, rows: &[WatchedMessage]) -> Result<Loaded, String> {
        let zone = self.config.zone;
        let mut starts: BTreeSet<DateTime<Utc>> = BTreeSet::new();
        for row in rows {
            let this = weeks::week_start(
                &row.created_at,
                zone,
                self.config.reset_weekday,
                self.config.reset_time,
            )
            .map_err(|error| error.to_string())?;
            let next = weeks::week_end(&this, zone).map_err(|error| error.to_string())?;
            starts.insert(utc(&this));
            starts.insert(utc(&next));
        }
        let snapshot = self
            .store
            .load(&Scope::Weeks(starts.into_iter().collect()))
            .await
            .map_err(|error| store_failure(SCHEDULE_UNREADABLE, &error))?;
        let first = rows.first().map(|row| row.created_at).unwrap_or_default();
        let history = self
            .store
            .channel_messages(channel_id, first - CONTEXT_WINDOW, false)
            .await
            .map_err(|error| store_failure(HISTORY_UNREADABLE, &error))?;
        Ok(Loaded {
            snapshot,
            history,
            members: self.guild.members(),
            bosses: self.guild.bosses(),
            channel_name: self.guild.channel_name(channel_id),
        })
    }

    /// One model call per piece of `rows` that fits the prompt budget (v4
    /// `fit_to_budget`); nothing is proposed or logged yet.
    pub(crate) async fn call_burst(
        &self,
        channel_id: &str,
        rows: Vec<WatchedMessage>,
    ) -> Vec<CallRecord> {
        let loaded = match self.load(channel_id, &rows).await {
            Ok(loaded) => loaded,
            Err(error) => {
                let mut record = self.record(&rows);
                record.fail(Failure::Failed, error);
                return vec![record];
            }
        };
        // Err high: the runner may add its schema instruction for a model
        // without structured output.
        let budget = prompt_budget(self.config.context_tokens)
            .saturating_sub(schema_instruction_tokens(&extraction_schema(None)));
        let chunks = split_until(
            &rows,
            |chunk| {
                let mut session = self.codec.open(&loaded.members);
                let prepared = self.prepare(channel_id, &loaded, chunk, session.as_mut());
                estimate_messages(&prepared.messages) <= budget
            },
            |row| row.created_at,
        );
        let mut records = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            records.push(self.call(channel_id, &loaded, &chunk).await);
        }
        records
    }

    // Rescan support: a window is read burst by burst and proposed once.

    pub(crate) fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    pub(crate) fn store(&self) -> &S {
        &self.store
    }

    pub(crate) fn guild(&self) -> &dyn Guild {
        self.guild.as_ref()
    }

    /// A channel's cached messages since `since`, processed or not (a rescan
    /// re-reads what the live pass handled): `(stored, gated)`.
    pub(crate) async fn gated_since(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> Result<(usize, Vec<WatchedMessage>), StoreError> {
        let rows = self
            .store
            .channel_messages(channel_id, since, false)
            .await?;
        let bosses = self.guild.bosses();
        let lexicon = BossLexicon::new(&bosses);
        let roster = self.roster_ids();
        let members: Vec<WatchedMessage> = rows
            .into_iter()
            .filter(|row| self.guild.has_role(&row.author_id))
            .collect();
        let stored = members.len();
        let gated = members
            .into_iter()
            .filter(|row| gated(row, self.guild.as_ref(), &lexicon, &roster).is_some())
            .collect();
        Ok((stored, gated))
    }

    /// Propose a whole rescan pass at once, consolidated (v4 one card).
    pub(crate) async fn commit_pass(
        &self,
        channel_id: &str,
        records: Vec<CallRecord>,
    ) -> PassReport {
        self.commit(channel_id, records, true).await
    }
}

pub(super) fn utc(at: &crate::domain::time::ZonedDateTime) -> DateTime<Utc> {
    at.to_fixed().with_timezone(&Utc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_failure_logs_a_fixed_sentence_not_the_store_text() {
        let error = StoreError::Backend("/private/var/db/kanade.sqlite3: disk I/O error".into());
        assert_eq!(
            store_failure(SCHEDULE_UNREADABLE, &error),
            SCHEDULE_UNREADABLE
        );
        assert_eq!(
            store_failure(HISTORY_UNREADABLE, &error),
            HISTORY_UNREADABLE
        );
    }
}
