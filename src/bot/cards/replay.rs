//! Unordered offline reactions: read both sides completely before deciding.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::PoisonError;

use serde_json::json;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::{Id, marker::UserMarker};

use super::react::{CHANGED_SINCE_CARD, OUT_OF_DATE_NOTICE};
use super::{CardDesk, CardReaction};
use crate::bot::delivery::AlertSink;
use crate::bot::events::RsvpAnswer;
use crate::bot::ids::{id_text, parse_id};
use crate::bot::transport::{ChannelId, DiscordTransport, MAX_REACTIONS_PAGE, MessageId, Outcome};
use crate::domain::drafts::{DraftStatus, ProposalStore};
use crate::domain::notify::{DeclineNoticeStore, DeliveryJournal};
use crate::domain::proposals::{ProposalCardStore, StoredCard};
use crate::domain::scheduler::{IdSource, ScheduleStore};
use crate::runtime::logging;

pub const OFFLINE_CONFLICT_NOTICE: &str = "⚠️ Both ✅ and ❌ were added while Kanade was offline. An approver should remove and re-add the intended one.";

// A full cap is incomplete, never evidence that the opposite answer is absent.
const MAX_REACTION_PAGES: usize = 100;
const MAX_LIVE_REACTION_RETRIES: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReplayReport {
    pub messages: usize,
    pub approved: usize,
    pub rejected: usize,
    pub conflicts: usize,
    pub skipped: usize,
    pub aborted: bool,
}

/// Give queued gateway decisions precedence over an HTTP snapshot.
pub(crate) trait ReplayLive: Send {
    fn drain(&mut self, message_id: &str) -> impl Future<Output = bool> + Send;
}

impl ReplayLive for () {
    async fn drain(&mut self, _message_id: &str) -> bool {
        false
    }
}

impl<S, T, I, A> CardDesk<S, T, I, A>
where
    S: ScheduleStore
        + DeclineNoticeStore
        + ProposalStore
        + ProposalCardStore
        + DeliveryJournal
        + Send
        + Sync,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
{
    /// Called only by the sequential reaction worker after roster reconciliation.
    /// `current` cancels stale generations/shutdown between HTTP calls and effects.
    /// Does not invoke the chat driver's rejection follow-up port.
    pub async fn replay_reactions(
        &self,
        self_id: Id<UserMarker>,
        current: impl Fn() -> bool + Send + Sync,
    ) -> ReplayReport {
        self.replay_reactions_with(self_id, current, &mut ()).await
    }

    pub(crate) async fn replay_reactions_with(
        &self,
        self_id: Id<UserMarker>,
        current: impl Fn() -> bool + Send + Sync,
        live_reactions: &mut impl ReplayLive,
    ) -> ReplayReport {
        let mut report = ReplayReport::default();
        let live = match self.store.list_proposals(true).await {
            Ok(live) => live,
            Err(_) => {
                skipped(None, "list_proposals");
                report.aborted = true;
                finished(&report);
                return report;
            }
        };
        let mut messages: BTreeMap<String, Vec<StoredCard>> = BTreeMap::new();
        // Load per proposal: one broken row must not stop the other cards.
        for proposal in live
            .into_iter()
            .filter(|row| row.draft.status == DraftStatus::Submitted)
        {
            match self.store.load_cards(&[proposal.draft.id]).await {
                Ok(cards) => {
                    for card in cards {
                        if let Some(message) = &card.message_id {
                            messages.entry(message.clone()).or_default().push(card);
                        }
                    }
                }
                Err(_) => skipped(None, "load_card"),
            }
        }
        'messages: for (message, cards) in messages {
            if !current() {
                report.aborted = true;
                break;
            }
            report.messages += 1;
            let mut changed_note = false;
            let mut stale = false;
            let mut reported_conflicts = BTreeSet::new();
            'retry: for retry in 0..=MAX_LIVE_REACTION_RETRIES {
                if live_reactions.drain(&message).await {
                    live_retry(&message, retry, &mut report);
                    continue 'retry;
                }
                let (Some(channel), Some(message_id)) =
                    (parse_id(&cards[0].channel_id), parse_id(&message))
                else {
                    skipped(Some(&message), "invalid_id");
                    report.skipped += 1;
                    continue 'messages;
                };
                let (yes, no) = match self
                    .read_answers(channel, message_id, self_id, &current)
                    .await
                {
                    Ok(answers) => answers,
                    Err(reason) => {
                        if !current() {
                            report.aborted = true;
                            break 'messages;
                        }
                        skipped(Some(&message), &reason);
                        report.skipped += 1;
                        continue 'messages;
                    }
                };
                if live_reactions.drain(&message).await {
                    live_retry(&message, retry, &mut report);
                    continue 'retry;
                }
                let reactors: Vec<_> = yes.union(&no).copied().collect();
                let approvers: Vec<_> = reactors
                    .iter()
                    .map(|user| self.authority.approver(&id_text(*user)))
                    .collect();
                // Preflight the whole message, including authority for each proposal.
                // A failure must not leave one sibling decided from a partial read.
                let mut choices = Vec::new();
                let mut failed = false;
                for card in &cards {
                    let allowed = match self
                        .service(self.now())
                        .proposal_answer_authority(&card.proposal_id, &approvers)
                        .await
                    {
                        Ok(allowed) => allowed,
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    };
                    choices.push([&yes, &no].map(|users| {
                        reactors
                            .iter()
                            .zip(&allowed)
                            .find(|(user, allowed)| **allowed && users.contains(user))
                            .map(|(user, _)| id_text(*user))
                    }));
                }
                if !current() {
                    report.aborted = true;
                    break 'messages;
                }
                if failed {
                    skipped(Some(&message), "authority");
                    report.skipped += 1;
                    continue 'messages;
                }
                if live_reactions.drain(&message).await {
                    live_retry(&message, retry, &mut report);
                    continue 'retry;
                }
                if !current() {
                    report.aborted = true;
                    break 'messages;
                }
                {
                    let mut conflicts = self
                        .replay_conflicts
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    for (card, choice) in cards.iter().zip(&choices) {
                        if choice[0].is_some() && choice[1].is_some() {
                            conflicts.insert(card.proposal_id.clone());
                            changed_note = true;
                            if reported_conflicts.insert(card.proposal_id.clone()) {
                                report.conflicts += 1;
                                logging::event(
                                    "WARN",
                                    "proposal_replay_conflict",
                                    json!({"message_id": message, "proposal_id": card.proposal_id}),
                                );
                            }
                        } else {
                            changed_note |= conflicts.remove(&card.proposal_id);
                        }
                    }
                }
                let mut groups: Vec<(String, RsvpAnswer, Vec<StoredCard>)> = Vec::new();
                for (card, [yes, no]) in cards.iter().cloned().zip(choices) {
                    let (user, answer) = match (yes, no) {
                        (Some(user), None) => (user, RsvpAnswer::Yes),
                        (None, Some(user)) => (user, RsvpAnswer::No),
                        _ => continue,
                    };
                    if let Some((_, _, group)) = groups
                        .iter_mut()
                        .find(|(who, side, _)| *who == user && *side == answer)
                    {
                        group.push(card);
                    } else {
                        groups.push((user, answer, vec![card]));
                    }
                }
                for (user, answer, cards) in groups {
                    if !current() {
                        report.aborted = true;
                        break 'messages;
                    }
                    if live_reactions.drain(&message).await {
                        live_retry(&message, retry, &mut report);
                        continue 'retry;
                    }
                    if !current() {
                        report.aborted = true;
                        break 'messages;
                    }
                    let result = self.answer_cards(&message, &user, answer, &cards).await;
                    let result = match &result {
                        CardReaction::Approved { approved, problems } => {
                            report.approved += approved.len();
                            stale |= problems.iter().any(|problem| problem == CHANGED_SINCE_CARD);
                            "approved"
                        }
                        CardReaction::Rejected { proposal_ids } => {
                            report.rejected += proposal_ids.len();
                            "rejected"
                        }
                        _ => "ignored",
                    };
                    logging::event(
                        "INFO",
                        "proposal_replay_answer",
                        json!({"message_id": message, "proposal_ids": cards.iter().map(|card| &card.proposal_id).collect::<Vec<_>>(), "answer": format!("{answer:?}"), "result": result}),
                    );
                }
                let extra: &[&str] = if stale { &[OUT_OF_DATE_NOTICE] } else { &[] };
                if (changed_note || stale) && current() && !self.refresh_with(&message, extra).await
                {
                    skipped(Some(&message), "refresh_note");
                }
                break 'retry;
            }
        }
        report.aborted |= !current();
        finished(&report);
        report
    }

    async fn read_answers(
        &self,
        channel: ChannelId,
        message: MessageId,
        self_id: Id<UserMarker>,
        current: &impl Fn() -> bool,
    ) -> Result<(BTreeSet<Id<UserMarker>>, BTreeSet<Id<UserMarker>>), String> {
        let mut answers = [BTreeSet::new(), BTreeSet::new()];
        for (emoji, users) in ["✅", "❌"].into_iter().zip(&mut answers) {
            // Live gateway reactions include super reactions too.
            for kind in [ReactionType::Normal, ReactionType::Burst] {
                let mut after = None;
                let mut complete = false;
                for _ in 0..MAX_REACTION_PAGES {
                    if !current() {
                        return Err("stale_generation".into());
                    }
                    let outcome = self
                        .transport
                        .reaction_users(channel, message, emoji, kind, after, MAX_REACTIONS_PAGE)
                        .await;
                    let page = match outcome {
                        Outcome::Delivered(page) => page,
                        outcome => return Err(outcome.failure_label().unwrap_or_default()),
                    };
                    if page.len() > usize::from(MAX_REACTIONS_PAGE)
                        || page
                            .iter()
                            .any(|id| after.is_some_and(|after| *id <= after))
                    {
                        return Err("invalid_page".into());
                    }
                    let next = page.iter().max().copied();
                    users.extend(page.iter().filter(|id| **id != self_id).copied());
                    if page.len() < usize::from(MAX_REACTIONS_PAGE) {
                        complete = true;
                        break;
                    }
                    after = next;
                }
                if !complete {
                    return Err("page_cap".into());
                }
            }
        }
        let [yes, no] = answers;
        Ok((yes, no))
    }
}

fn live_retry(message: &str, retry: usize, report: &mut ReplayReport) {
    if retry == MAX_LIVE_REACTION_RETRIES {
        skipped(Some(message), "live_reaction_retry_cap");
        report.skipped += 1;
    }
}

fn finished(report: &ReplayReport) {
    logging::event(
        if report.aborted { "WARN" } else { "INFO" },
        if report.aborted {
            "proposal_replay_aborted"
        } else {
            "proposal_replay_complete"
        },
        json!({"messages": report.messages, "approved": report.approved, "rejected": report.rejected, "conflicts": report.conflicts, "skipped": report.skipped}),
    );
}

fn skipped(message: Option<&str>, reason: &str) {
    logging::event(
        "WARN",
        "proposal_replay_skipped",
        json!({"message_id": message, "reason": reason}),
    );
}
