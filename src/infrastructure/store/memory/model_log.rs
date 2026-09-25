//! In-memory `ModelLogStore` mirroring the SQLite queries (ordering, ASCII
//! case folding, microsecond instants, derived filter sets).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use super::{MemoryScheduleStore, micros};
use crate::domain::model_log::{
    AllowanceOverride, ChatFilter, ChatInteraction, ChatOutcome, ExtractionFilter, ExtractionLog,
    LogCursor, LogFacets, LogPage, MessageUpsert, ModelLogStore, PruneCounts, ReadMessage,
    RescanJob, WatchedMessage, in_order, page_size,
};
use crate::domain::scheduler::StoreError;

#[derive(Debug, Default)]
pub(super) struct LogTables {
    messages: BTreeMap<String, WatchedMessage>,
    extractions: BTreeMap<String, ExtractionLog>,
    chats: BTreeMap<String, ChatInteraction>,
    rescans: BTreeMap<String, RescanJob>,
    allowances: BTreeMap<String, AllowanceOverride>,
    tips: BTreeSet<(String, DateTime<Utc>)>,
}

fn optional(at: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    at.map(micros)
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack
        .to_ascii_lowercase()
        .contains(&needle.to_ascii_lowercase())
}

fn after_cursor(at: DateTime<Utc>, id: &str, cursor: Option<&LogCursor>) -> bool {
    cursor.is_none_or(|cursor| {
        let cursor_at = micros(cursor.at);
        at < cursor_at || (at == cursor_at && id < cursor.id.as_str())
    })
}

fn in_range(at: DateTime<Utc>, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>) -> bool {
    from.is_none_or(|from| at >= micros(from)) && to.is_none_or(|to| at < micros(to))
}

fn page<T>(mut items: Vec<T>, limit: u32, key: impl Fn(&T) -> LogCursor) -> LogPage<T> {
    let size = page_size(limit) as usize;
    let next = if items.len() > size {
        items.truncate(size);
        items.last().map(key)
    } else {
        None
    };
    LogPage { items, next }
}

fn sorted(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl MemoryScheduleStore {
    fn logs(&self) -> std::sync::MutexGuard<'_, LogTables> {
        self.logs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ModelLogStore for MemoryScheduleStore {
    async fn upsert_message(&self, message: WatchedMessage) -> Result<MessageUpsert, StoreError> {
        let mut logs = self.logs();
        match logs.messages.get_mut(&message.id) {
            None => {
                let message = WatchedMessage {
                    created_at: micros(message.created_at),
                    edited_at: optional(message.edited_at),
                    processed_at: optional(message.processed_at),
                    ..message
                };
                logs.messages.insert(message.id.clone(), message);
                Ok(MessageUpsert::Inserted)
            }
            Some(stored) if stored.content == message.content => Ok(MessageUpsert::Unchanged),
            Some(stored) => {
                stored.content = message.content;
                stored.edited_at = optional(message.edited_at);
                stored.processed_at = None;
                Ok(MessageUpsert::Edited)
            }
        }
    }

    async fn mark_processed(&self, ids: &[String], at: DateTime<Utc>) -> Result<u64, StoreError> {
        let mut logs = self.logs();
        let mut done = 0;
        for id in ids.iter().collect::<BTreeSet<_>>() {
            if let Some(message) = logs.messages.get_mut(id) {
                message.processed_at = Some(micros(at));
                done += 1;
            }
        }
        Ok(done)
    }

    async fn mark_read(&self, read: &[ReadMessage], at: DateTime<Utc>) -> Result<u64, StoreError> {
        let mut logs = self.logs();
        let mut done = 0;
        let mut seen = BTreeSet::new();
        for entry in read {
            if !seen.insert(&entry.id) {
                continue;
            }
            if let Some(message) = logs.messages.get_mut(&entry.id)
                && message.content == entry.content
            {
                message.processed_at = Some(micros(at));
                done += 1;
            }
        }
        Ok(done)
    }

    async fn delete_message(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self.logs().messages.remove(id).is_some())
    }

    async fn channel_messages(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
        unprocessed_only: bool,
    ) -> Result<Vec<WatchedMessage>, StoreError> {
        let since = micros(since);
        let mut found: Vec<WatchedMessage> = self
            .logs()
            .messages
            .values()
            .filter(|message| {
                message.channel_id == channel_id
                    && message.created_at >= since
                    && (!unprocessed_only || message.processed_at.is_none())
            })
            .cloned()
            .collect();
        found.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
        Ok(found)
    }

    async fn messages_by_ids(&self, ids: &[String]) -> Result<Vec<WatchedMessage>, StoreError> {
        let found = self
            .logs()
            .messages
            .values()
            .filter(|message| ids.contains(&message.id))
            .cloned()
            .collect();
        Ok(in_order(ids, found))
    }

    async fn record_extraction(&self, log: ExtractionLog) -> Result<(), StoreError> {
        log.check_shape()?;
        let mut logs = self.logs();
        if logs.extractions.contains_key(&log.id) {
            return Err(StoreError::Constraint(format!(
                "extraction {} exists",
                log.id
            )));
        }
        let log = ExtractionLog {
            at: micros(log.at),
            ..log
        };
        logs.extractions.insert(log.id.clone(), log);
        Ok(())
    }

    async fn load_extraction(&self, id: &str) -> Result<Option<ExtractionLog>, StoreError> {
        Ok(self.logs().extractions.get(id).cloned())
    }

    async fn list_extractions(
        &self,
        filter: &ExtractionFilter,
    ) -> Result<LogPage<ExtractionLog>, StoreError> {
        let mut found: Vec<ExtractionLog> =
            self.logs()
                .extractions
                .values()
                .filter(|log| {
                    filter
                        .model
                        .as_ref()
                        .is_none_or(|model| &log.model == model)
                        && in_range(log.at, filter.from, filter.to)
                        && (filter.outcomes.is_empty() || filter.outcomes.contains(&log.outcome))
                        && filter
                            .channel
                            .as_ref()
                            .is_none_or(|channel| log.channel_id.as_ref() == Some(channel))
                        && filter
                            .member
                            .as_ref()
                            .is_none_or(|member| log.member_ids.contains(member))
                        && filter.q.as_ref().is_none_or(|q| {
                            contains(&log.prompt, q) || contains(&log.raw_response, q)
                        })
                        && after_cursor(log.at, &log.id, filter.cursor.as_ref())
                })
                .cloned()
                .collect();
        found.sort_by(|a, b| (b.at, &b.id).cmp(&(a.at, &a.id)));
        found.truncate(page_size(filter.limit) as usize + 1);
        if filter.omit_bodies {
            for log in &mut found {
                log.prompt.clear();
                log.raw_response.clear();
            }
        }
        Ok(page(found, filter.limit, |log| LogCursor {
            at: log.at,
            id: log.id.clone(),
        }))
    }

    async fn extraction_facets(&self) -> Result<LogFacets, StoreError> {
        let logs = self.logs();
        let all = || logs.extractions.values();
        Ok(LogFacets {
            total: all().count() as u64,
            models: sorted(all().map(|log| log.model.clone())),
            tools: Vec::new(),
            outcomes: sorted(all().map(|log| log.outcome.as_str().to_owned())),
            channels: sorted(all().filter_map(|log| log.channel_id.clone())),
        })
    }

    async fn record_chat(&self, interaction: ChatInteraction) -> Result<(), StoreError> {
        interaction.check_shape()?;
        let mut logs = self.logs();
        if logs.chats.contains_key(&interaction.id) {
            return Err(StoreError::Constraint(format!(
                "chat interaction {} exists",
                interaction.id
            )));
        }
        let interaction = ChatInteraction {
            at: micros(interaction.at),
            ..interaction
        };
        logs.chats.insert(interaction.id.clone(), interaction);
        Ok(())
    }

    async fn load_chat(&self, id: &str) -> Result<Option<ChatInteraction>, StoreError> {
        Ok(self.logs().chats.get(id).cloned())
    }

    async fn list_chats(
        &self,
        filter: &ChatFilter,
    ) -> Result<LogPage<ChatInteraction>, StoreError> {
        let outcome = |chat: &ChatInteraction| {
            filter.outcomes.is_empty()
                || filter.outcomes.contains(&chat.outcome)
                || (chat.clean_retry && filter.outcomes.contains(&ChatOutcome::CleanRetry))
                || (chat.withheld && filter.outcomes.contains(&ChatOutcome::Withheld))
        };
        let mut found: Vec<ChatInteraction> = self
            .logs()
            .chats
            .values()
            .filter(|chat| {
                filter
                    .model
                    .as_ref()
                    .is_none_or(|model| chat.rounds.iter().any(|round| &round.model == model))
                    && in_range(chat.at, filter.from, filter.to)
                    && outcome(chat)
                    && filter
                        .channel
                        .as_ref()
                        .is_none_or(|channel| chat.channel_id.as_ref() == Some(channel))
                    && filter
                        .member
                        .as_ref()
                        .is_none_or(|member| chat.member_id.as_ref() == Some(member))
                    && filter
                        .q
                        .as_ref()
                        .is_none_or(|q| contains(&chat.question, q) || contains(&chat.reply, q))
                    && filter.tool.as_ref().is_none_or(|tool| {
                        chat.rounds.iter().any(|round| round.tools.contains(tool))
                    })
                    && filter
                        .min_ms
                        .is_none_or(|min| chat.latency_ms.is_some_and(|ms| ms >= min))
                    && after_cursor(chat.at, &chat.id, filter.cursor.as_ref())
            })
            .cloned()
            .collect();
        found.sort_by(|a, b| (b.at, &b.id).cmp(&(a.at, &a.id)));
        found.truncate(page_size(filter.limit) as usize + 1);
        Ok(page(found, filter.limit, |chat| LogCursor {
            at: chat.at,
            id: chat.id.clone(),
        }))
    }

    async fn chat_facets(&self) -> Result<LogFacets, StoreError> {
        let logs = self.logs();
        let all = || logs.chats.values();
        let rounds = || all().flat_map(|chat| chat.rounds.iter());
        let mut outcomes: Vec<String> =
            all().map(|chat| chat.outcome.as_str().to_owned()).collect();
        if all().any(|chat| chat.clean_retry) {
            outcomes.push(ChatOutcome::CleanRetry.as_str().to_owned());
        }
        if all().any(|chat| chat.withheld) {
            outcomes.push(ChatOutcome::Withheld.as_str().to_owned());
        }
        Ok(LogFacets {
            total: all().count() as u64,
            models: sorted(rounds().map(|round| round.model.clone())),
            tools: sorted(rounds().flat_map(|round| round.tools.iter().cloned())),
            outcomes: sorted(outcomes),
            channels: sorted(all().filter_map(|chat| chat.channel_id.clone())),
        })
    }

    async fn prune_model_logs(&self, before: DateTime<Utc>) -> Result<PruneCounts, StoreError> {
        let before = micros(before);
        let mut logs = self.logs();
        let extractions = logs.extractions.len();
        logs.extractions.retain(|_, log| log.at >= before);
        let chats = logs.chats.len();
        logs.chats.retain(|_, chat| chat.at >= before);
        let messages = logs.messages.len();
        logs.messages
            .retain(|_, message| message.processed_at.is_none() || message.created_at >= before);
        Ok(PruneCounts {
            extractions: (extractions - logs.extractions.len()) as u64,
            chats: (chats - logs.chats.len()) as u64,
            messages: (messages - logs.messages.len()) as u64,
        })
    }

    async fn insert_rescan_job(&self, job: RescanJob) -> Result<(), StoreError> {
        job.check_shape()?;
        let mut logs = self.logs();
        if logs.rescans.contains_key(&job.id) {
            return Err(StoreError::Constraint(format!(
                "rescan job {} exists",
                job.id
            )));
        }
        let job = RescanJob {
            created_at: micros(job.created_at),
            started_at: optional(job.started_at),
            finished_at: optional(job.finished_at),
            ..job
        };
        logs.rescans.insert(job.id.clone(), job);
        Ok(())
    }

    async fn update_rescan_job(&self, job: RescanJob) -> Result<bool, StoreError> {
        job.check_shape()?;
        let mut logs = self.logs();
        let Some(stored) = logs.rescans.get_mut(&job.id) else {
            return Ok(false);
        };
        if stored.status.is_final() {
            return Ok(false);
        }
        stored.status = job.status;
        stored.started_at = optional(job.started_at);
        stored.finished_at = optional(job.finished_at);
        stored.results = job.results;
        stored.error = job.error;
        Ok(true)
    }

    async fn load_rescan_job(&self, id: &str) -> Result<Option<RescanJob>, StoreError> {
        Ok(self.logs().rescans.get(id).cloned())
    }

    async fn recent_rescan_jobs(&self, limit: u32) -> Result<Vec<RescanJob>, StoreError> {
        let mut jobs: Vec<RescanJob> = self.logs().rescans.values().cloned().collect();
        jobs.sort_by(|a, b| (b.created_at, &b.id).cmp(&(a.created_at, &a.id)));
        jobs.truncate(limit as usize);
        Ok(jobs)
    }

    async fn set_allowance_override(&self, entry: AllowanceOverride) -> Result<(), StoreError> {
        entry.check_shape()?;
        let entry = AllowanceOverride {
            updated_at: micros(entry.updated_at),
            ..entry
        };
        self.logs()
            .allowances
            .insert(entry.member_id.clone(), entry);
        Ok(())
    }

    async fn clear_allowance_override(&self, member_id: &str) -> Result<bool, StoreError> {
        Ok(self.logs().allowances.remove(member_id).is_some())
    }

    async fn allowance_overrides(&self) -> Result<Vec<AllowanceOverride>, StoreError> {
        Ok(self.logs().allowances.values().cloned().collect())
    }

    async fn claim_tip(
        &self,
        member_id: &str,
        week: DateTime<Utc>,
        _at: DateTime<Utc>,
    ) -> Result<bool, StoreError> {
        Ok(self
            .logs()
            .tips
            .insert((member_id.to_owned(), micros(week))))
    }

    async fn release_tip(&self, member_id: &str, week: DateTime<Utc>) -> Result<bool, StoreError> {
        Ok(self
            .logs()
            .tips
            .remove(&(member_id.to_owned(), micros(week))))
    }
}
