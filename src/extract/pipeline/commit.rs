//! Turning a pass's planned changes into proposals, chat answers and one
//! card, then writing one extraction log row per call (v4 `apply_plan`,
//! `_record` and `set_extraction_amendments`). Proposals go only through the
//! scheduler's propose API; nothing here writes schedule rows.

use std::collections::{BTreeSet, HashSet};

use serde_json::json;

use super::call::{CallRecord, Failure, Kept};
use super::extractor::{Extractor, PassReport, utc};
use super::outcome::extraction_outcome;
use super::ports::{Card, CardEntry, ChatAnswer, Outbox, Proposer, RedirectOffer};
use crate::domain::drafts::ProposalSource;
use crate::domain::model_log::{ExtractionLog, ModelLogStore};
use crate::domain::proposals::{ChangeKind, Payload as ChangePayload, ProposedChange};
use crate::domain::schedule::RsvpState;
use crate::domain::scheduler::{ProposalRequest, ScheduleStore, Supersede, SupersedeScope};
use crate::extract::AmendmentKind;
use crate::extract::plan::{Payload, consolidate};
use crate::infrastructure::llm::LlmProvider;

fn change_kind(kind: AmendmentKind) -> ChangeKind {
    ChangeKind::parse(kind.as_str()).expect("amendment and change kinds share names")
}

/// v4 `_record`'s row for one kept change.
fn proposed_change(kept: &Kept, channel_id: &str) -> ProposedChange {
    ProposedChange {
        kind: change_kind(kept.amendment.kind),
        run_id: kept.run.as_ref().map(|run| run.id.clone()),
        channel_id: Some(channel_id.to_owned()),
        bosses: kept.amendment.bosses.clone(),
        participants: kept.amendment.participants.clone(),
        new_datetime: kept.resolved.at.as_ref().map(utc),
        rsvp: kept.amendment.rsvp,
        payload: match &kept.payload {
            Payload::Empty => ChangePayload::None,
            Payload::Fix { weekday, time } => ChangePayload::Fix {
                weekday: Some(*weekday),
                time: Some(*time),
            },
            Payload::Split {
                bosses,
                participants,
            } => ChangePayload::Split {
                bosses: Some(bosses.clone()),
                participants: participants.clone(),
            },
            Payload::Sub { remove, add } => ChangePayload::Sub {
                remove: remove.clone(),
                add: add.clone(),
            },
        },
    }
}

/// What one pass's changes retire: the run, else the new boss set.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Target {
    Run(String),
    Bosses(Vec<String>),
    None,
}

fn target(kept: &Kept) -> Target {
    if let Some(run) = &kept.run {
        return Target::Run(run.id.clone());
    }
    let bosses: BTreeSet<String> = kept.amendment.bosses.iter().cloned().collect();
    if bosses.is_empty() {
        Target::None
    } else {
        Target::Bosses(bosses.into_iter().collect())
    }
}

/// Kept changes with the record each came from; several calls' changes are
/// consolidated to the last word per target (v4 rescans and split bursts).
fn collect(records: &[CallRecord], consolidated: bool) -> Vec<(usize, Kept)> {
    let entries: Vec<(usize, Kept)> = records
        .iter()
        .enumerate()
        .filter(|(_, record)| record.ok())
        .flat_map(|(index, record)| record.kept.iter().map(move |kept| (index, kept.clone())))
        .collect();
    if !consolidated {
        return entries;
    }
    let chosen = consolidate(entries.iter().map(|(_, kept)| kept.planned()).collect());
    chosen
        .into_iter()
        .map(|planned| {
            // `consolidate` keeps the latest of equal candidates.
            let origin = entries
                .iter()
                .rev()
                .find(|(_, kept)| {
                    kept.amendment == planned.amendment
                        && kept.resolved == planned.resolved
                        && kept.run.as_ref().map(|run| &run.id) == planned.run.map(|run| &run.id)
                })
                .map_or(0, |(index, _)| *index);
            (origin, Kept::from_planned(planned))
        })
        .collect()
}

impl<S, P, X, O> Extractor<S, P, X, O>
where
    S: ScheduleStore + ModelLogStore + Send + Sync,
    P: LlmProvider,
    X: Proposer,
    O: Outbox,
{
    /// Propose what the pass kept, hand over chat answers and one card, mark
    /// the answered calls' messages processed and log every call.
    pub(super) async fn commit(
        &self,
        channel_id: &str,
        mut records: Vec<CallRecord>,
        consolidated: bool,
    ) -> PassReport {
        let mut report = PassReport::default();
        let entries = collect(&records, consolidated);

        let mut answers: Vec<ChatAnswer> = Vec::new();
        let mut proposals: Vec<(usize, Kept, ProposedChange)> = Vec::new();
        for (origin, kept) in entries {
            if kept.amendment.kind == AmendmentKind::Rsvp {
                // "maybe" is recorded by nobody, as v4.
                let (Some(run), Some(state @ (RsvpState::Yes | RsvpState::No))) =
                    (&kept.run, kept.amendment.rsvp)
                else {
                    continue;
                };
                answers.push(ChatAnswer {
                    channel_id: channel_id.to_owned(),
                    run_id: run.id.clone(),
                    user_ids: kept.amendment.participants.clone(),
                    state,
                });
                continue;
            }
            let change = proposed_change(&kept, channel_id);
            let authors: Vec<String> = kept
                .amendment
                .evidence_message_ids
                .iter()
                .filter_map(|id| records[origin].authors.get(id).cloned())
                .collect();
            let offer = RedirectOffer {
                channel_id: channel_id.to_owned(),
                change: change.clone(),
                authors,
            };
            if self.outbox.redirect(&offer).await {
                records[origin].redirected += 1;
                continue;
            }
            proposals.push((origin, kept, change));
        }

        // v4 `_record`: every older card about these targets retires before
        // anything is written, so this pass never retires its own siblings.
        let mut superseded: Vec<String> = Vec::new();
        let targets: BTreeSet<Target> = proposals.iter().map(|(_, kept, _)| target(kept)).collect();
        for goal in &targets {
            let scope = match goal {
                Target::Run(run_id) => SupersedeScope {
                    run_id: Some(run_id),
                    channel_id: Some(channel_id),
                    bosses: &[],
                    keep: None,
                    from_channel: Some(channel_id),
                    by: ProposalSource::Extraction,
                },
                Target::Bosses(bosses) => SupersedeScope {
                    run_id: None,
                    channel_id: Some(channel_id),
                    bosses,
                    keep: None,
                    from_channel: None,
                    by: ProposalSource::Extraction,
                },
                Target::None => continue,
            };
            match self.proposer.supersede(scope).await {
                Ok(ids) => superseded.extend(ids),
                Err(error) => report.errors.push(format!("supersede: {error}")),
            }
        }

        let mut keyed: HashSet<Target> = HashSet::new();
        let mut card: Vec<CardEntry> = Vec::new();
        for (origin, kept, change) in proposals {
            // The first change per target stores the supersede key; a second
            // one (a `sub` beside a `move`) must not retire its sibling.
            let supersede = if keyed.insert(target(&kept)) {
                Supersede::Older
            } else {
                Supersede::Keep
            };
            let request = ProposalRequest {
                change,
                source: ProposalSource::Extraction,
                source_id: records[origin].log_id.clone(),
                supersede,
            };
            match self.proposer.propose(request).await {
                Ok(proposed) => {
                    let id = proposed.proposal.id.clone();
                    superseded.extend(proposed.superseded);
                    records[origin].proposal_ids.push(id.clone());
                    report.proposals.push(id.clone());
                    card.push(CardEntry {
                        proposal_id: id,
                        kind: kept.amendment.kind,
                        run_id: kept.run.as_ref().map(|run| run.id.clone()),
                        summary: kept.summary.clone(),
                        is_question: kept.amendment.is_question,
                        needs_answer: kept.planned().needs_answer(),
                        confidence: kept.amendment.confidence,
                        also_mentioned: kept.also_mentioned.clone(),
                        day_ref: kept.amendment.day_ref.clone(),
                        time_ref: kept.amendment.time_ref.clone(),
                        evidence_message_ids: kept.amendment.evidence_message_ids.clone(),
                    });
                }
                Err(error) => {
                    let text = format!("{}: {error}", kept.amendment.kind.as_str());
                    records[origin].refusals.push(text.clone());
                    report.refused.push(text);
                }
            }
        }

        let fresh: HashSet<&str> = report.proposals.iter().map(String::as_str).collect();
        superseded.retain(|id| !fresh.contains(id.as_str()));
        superseded.sort();
        superseded.dedup();
        if !card.is_empty() || !superseded.is_empty() {
            self.outbox
                .card(Card {
                    channel_id: channel_id.to_owned(),
                    entries: card,
                    superseded,
                })
                .await;
        }
        report.answers = answers.len();
        if !answers.is_empty() {
            self.outbox.answers(answers).await;
        }

        let now = self.clock.now();
        let consumed: Vec<String> = records
            .iter()
            .filter(|record| record.ok())
            .flat_map(|record| record.message_ids.iter().cloned())
            .collect();
        if !consumed.is_empty()
            && let Err(error) = self.store.mark_processed(&consumed, now).await
        {
            report.errors.push(error.to_string());
        }

        for record in records {
            report.dropped += record.dropped;
            report.stale += record.stale;
            report.redirected += record.redirected;
            if let Some(Failure::TurnedAway { retry_at }) = record.failure {
                report.turned_away.extend(record.burst.iter().cloned());
                report.retry_at = report.retry_at.max(retry_at);
            }
            if let Some(error) = &record.error {
                report.errors.push(error.clone());
            }
            let error = record.error.clone().or_else(|| {
                (!record.refusals.is_empty())
                    .then(|| format!("refused up front: {}", record.refusals.join("; ")))
            });
            let log = ExtractionLog {
                id: record.log_id.clone(),
                at: record.at,
                channel_id: Some(channel_id.to_owned()),
                member_ids: record.member_ids,
                model: record.model,
                reasoning: self
                    .config
                    .reasoning
                    .map(|effort| effort.as_str().to_owned()),
                prompt: record.prompt,
                raw_response: record.raw,
                latency_ms: record.latency_ms,
                request_count: record.requests,
                outcome: extraction_outcome(
                    record.failure,
                    record.proposal_ids.len(),
                    record.redirected,
                ),
                error,
                guardrail: json!({}),
                message_ids: record.message_ids,
                proposal_ids: record.proposal_ids,
            };
            match self.store.record_extraction(log).await {
                Ok(()) => report.logs.push(record.log_id),
                Err(error) => report.errors.push(error.to_string()),
            }
        }
        report
    }
}
