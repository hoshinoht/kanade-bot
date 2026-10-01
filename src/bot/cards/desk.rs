//! Posting and refreshing proposal cards. Every post is journalled (claim →
//! send → bind or resolve, through the delivery [`Executor`]); an ambiguous
//! send stays held and is never replayed. A card is re-rendered from its
//! stored details and its proposals' states, so it can be refreshed after a
//! restart; allowed mentions are empty (names only).

use std::collections::BTreeMap;
use std::sync::Arc;

use chrono::{DateTime, Timelike, Utc};
use chrono_tz::Tz;
use twilight_model::channel::message::Embed;
use twilight_model::channel::message::embed::{EmbedField, EmbedFooter};

use super::format::{
    Audience, CardView, SUPERSEDED_NOTICE, applied_notice, proposal_card, rejected_notice,
    unanswered,
};
use crate::api::state::DeclineRetraction;
use crate::bot::delivery::{
    AdminAlert, AlertSink, AlertThrottle, Executor, FixedClock, SendFailure, SendOutcome, StoreRef,
};
use crate::bot::ids::parse_id;
use crate::bot::mentions;
use crate::bot::transport::{DiscordTransport, MessageEdit, OutgoingMessage};
use crate::chat::nudge::render as render_tip;
use crate::domain::drafts::{DraftStatus, ProposalStore, SUPERSEDED};
use crate::domain::history::Actor;
use crate::domain::members::Directory;
use crate::domain::notify::{
    DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, JournalView, NotificationIntent,
    PlannedSend, SendDisposition,
};
use crate::domain::proposals::{
    Approver, CardDetails, CardPayload, Payload, ProposalCardStore, StoredCard,
};
use crate::domain::schedule::{Run, SchedulePolicy};
use crate::domain::scheduler::{
    Clock, IdSource, ScheduleStore, SchedulerService, Scope, StoreError,
};
use crate::extract::pipeline::{CardEntry, PostResult, Redirected};

/// The reacting member's standing, from the gateway (roles, Administrator,
/// guild owner) and the roster.
pub trait Authority: Send + Sync {
    fn approver(&self, user_id: &str) -> Approver;
}

#[derive(Clone, Debug)]
pub struct CardSettings {
    pub zone: Tz,
    pub policy: SchedulePolicy,
    /// This process, for delivery leases.
    pub instance_id: String,
}

/// Everything a [`CardDesk`] is built from.
pub struct DeskDeps<S, T, I, A> {
    pub store: Arc<S>,
    pub transport: Arc<T>,
    pub ids: I,
    pub clock: Arc<dyn Clock + Send + Sync>,
    pub directory: Arc<dyn Directory + Send + Sync>,
    pub authority: Arc<dyn Authority>,
    pub alerts: Arc<A>,
    pub decline_retraction: Option<DeclineRetraction>,
}

/// Posts, refreshes and answers proposal cards.
pub struct CardDesk<S, T, I, A> {
    pub(super) store: Arc<S>,
    pub(super) transport: Arc<T>,
    pub(super) ids: I,
    pub(super) clock: Arc<dyn Clock + Send + Sync>,
    pub(super) directory: Arc<dyn Directory + Send + Sync>,
    pub(super) authority: Arc<dyn Authority>,
    pub(super) alerts: Arc<A>,
    pub(super) decline_retraction: Option<DeclineRetraction>,
    throttle: AlertThrottle,
    pub(super) settings: CardSettings,
}

impl<S, T, I, A> std::fmt::Debug for CardDesk<S, T, I, A> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardDesk")
            .field("instance_id", &self.settings.instance_id)
            .finish_non_exhaustive()
    }
}

const WEEKDAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn hhmm(time: chrono::NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

fn weekday_index(weekday: chrono::Weekday) -> u8 {
    u8::try_from(weekday.num_days_from_monday()).unwrap_or_default()
}

/// A delivered, held or maybe-delivered post counts as posted.
/// A journal write that failed after Discord took (or may have taken) the
/// post still counts: the message may be visible.
fn posted(outcome: Option<Result<SendOutcome, SendFailure>>) -> PostResult {
    match outcome {
        Some(Ok(SendOutcome::Bound(_) | SendOutcome::Uncertain | SendOutcome::Suppressed)) => {
            PostResult::Posted
        }
        Some(Err(failure)) if failure.maybe_delivered => PostResult::Posted,
        _ => PostResult::NotPosted,
    }
}

pub(super) fn embed(view: &CardView) -> Embed {
    Embed {
        author: None,
        color: Some(view.colour),
        description: view.description.clone(),
        fields: view
            .fields
            .iter()
            .map(|(name, value)| EmbedField {
                inline: false,
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        footer: view.footer.as_ref().map(|text| EmbedFooter {
            icon_url: None,
            proxy_icon_url: None,
            text: text.clone(),
        }),
        image: None,
        kind: "rich".to_owned(),
        provider: None,
        thumbnail: None,
        timestamp: None,
        title: view.title.clone(),
        url: None,
        video: None,
    }
}

fn with_notices(content: &str, notices: &[String]) -> String {
    let mut text = content.to_owned();
    for notice in notices {
        text.push('\n');
        text.push_str(notice);
    }
    text
}

impl<S, T, I, A> CardDesk<S, T, I, A>
where
    S: ScheduleStore
        + crate::domain::notify::DeclineNoticeStore
        + ProposalStore
        + ProposalCardStore
        + DeliveryJournal
        + Send
        + Sync,
    T: DiscordTransport,
    I: IdSource + Clone + Send + Sync,
    A: AlertSink,
{
    pub fn new(deps: DeskDeps<S, T, I, A>, settings: CardSettings) -> Self {
        Self {
            store: deps.store,
            transport: deps.transport,
            ids: deps.ids,
            clock: deps.clock,
            directory: deps.directory,
            authority: deps.authority,
            alerts: deps.alerts,
            decline_retraction: deps.decline_retraction,
            throttle: AlertThrottle::new(),
            settings,
        }
    }

    pub(super) fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// A short-lived scheduler service on this desk's store and clock reading.
    pub(super) fn service(
        &self,
        now: DateTime<Utc>,
    ) -> SchedulerService<StoreRef<'_, S>, I, FixedClock> {
        // The service refuses a policy whose attendance mode differs from its own.
        SchedulerService::new(StoreRef(&*self.store), self.ids.clone(), FixedClock(now))
            .with_attendance(self.settings.policy.attendance)
    }

    pub(super) fn raise(&self, alert: AdminAlert, now: DateTime<Utc>) {
        if self.throttle.admit(&alert, now) {
            self.alerts.alert(alert);
        }
    }

    /// What a closing actor is called on a card.
    fn actor_name(&self, actor: Option<&Actor>) -> String {
        match actor {
            Some(Actor::Member { id } | Actor::Admin { id }) => self
                .directory
                .display_name(id)
                .unwrap_or_else(|| format!("<@{id}>")),
            Some(Actor::System { component }) => component.clone(),
            None => "someone".to_owned(),
        }
    }

    /// The card's details from a pipeline entry; a timing edit or removal
    /// names the timing's current slot (v4 `weekly_when`).
    async fn details_of(&self, entry: &CardEntry) -> CardDetails {
        let change = &entry.change;
        let mut payload = CardPayload::default();
        let timing = |id: &Option<String>| id.clone();
        let named = match &change.payload {
            Payload::Fix { weekday, time } => {
                payload.weekday = weekday.map(weekday_index);
                payload.time = time.map(hhmm);
                None
            }
            Payload::FixEdit {
                fixed_run_id,
                weekday,
                time,
                participants,
            } => {
                payload.op = Some("edit".into());
                payload.weekday = weekday.map(weekday_index);
                payload.time = time.map(hhmm);
                payload.participants = participants.clone();
                timing(fixed_run_id)
            }
            Payload::FixRemove { fixed_run_id } => {
                payload.op = Some("remove".into());
                timing(fixed_run_id)
            }
            Payload::Sub { remove, add } => {
                payload.remove = remove.clone();
                payload.add = add.clone();
                None
            }
            Payload::Split { .. } | Payload::None => None,
        };
        if let Some(fixed_id) = named
            && let Ok(snapshot) = self.store.load(&Scope::Weeks(Vec::new())).await
            && let Some(fixed) = snapshot.fixed_runs.iter().find(|row| row.id == fixed_id)
        {
            payload.weekly_when = Some(format!(
                "{} {}",
                WEEKDAY_NAMES[usize::from(weekday_index(fixed.weekday))],
                hhmm(fixed.time)
            ));
        }
        CardDetails {
            kind: change.kind,
            run_id: change.run_id.clone(),
            bosses: change.bosses.clone(),
            participants: change.participants.clone(),
            new_datetime: change.new_datetime,
            day_ref: entry.day_ref.clone(),
            time_ref: entry.time_ref.clone(),
            rsvp: change.rsvp.map(|state| state.as_str().to_owned()),
            is_question: entry.is_question,
            summary: Some(entry.summary.clone()).filter(|text| !text.is_empty()),
            also_mentioned: entry
                .also_mentioned
                .iter()
                .map(|kind| kind.as_str().to_owned())
                .collect(),
            confidence: entry.confidence,
            payload,
            evidence_message_ids: entry.evidence_message_ids.clone(),
            self_service: entry
                .self_service
                .as_ref()
                .map(|tip| render_tip(tip.lead_in.as_deref(), tip.link.purpose, &tip.link.url)),
        }
    }

    /// Names for everyone the card may show; it notifies nobody.
    fn audience(&self, cards: &[StoredCard], runs: &[Run]) -> Audience {
        let mut names = BTreeMap::new();
        let people = cards
            .iter()
            .flat_map(|card| {
                let payload = &card.details.payload;
                card.details
                    .participants
                    .iter()
                    .chain(&payload.participants)
                    .chain(&payload.remove)
                    .chain(&payload.add)
            })
            .chain(runs.iter().flat_map(|run| &run.participants));
        for id in people {
            if let Some(name) = self.directory.display_name(id) {
                names.insert(id.clone(), name);
            }
        }
        Audience {
            names,
            mentioned: Vec::new(),
        }
    }

    /// The card for these proposals against the runs as they stand now.
    pub(super) async fn view(&self, cards: &[StoredCard]) -> Result<CardView, StoreError> {
        let mut runs: Vec<Run> = Vec::new();
        for card in cards {
            let Some(run_id) = &card.details.run_id else {
                continue;
            };
            if runs.iter().any(|run| &run.id == run_id) {
                continue;
            }
            let snapshot = self.store.load(&Scope::Run(run_id.clone())).await?;
            runs.extend(snapshot.runs.into_iter().filter(|run| &run.id == run_id));
        }
        let details: Vec<&CardDetails> = cards.iter().map(|card| &card.details).collect();
        let run_refs: Vec<&Run> = runs.iter().collect();
        let waiting = unanswered(&details, &run_refs);
        let by_id: BTreeMap<String, &Run> = runs.iter().map(|run| (run.id.clone(), run)).collect();
        let confidence = details
            .iter()
            .map(|details| details.confidence)
            .fold(None, |low: Option<f64>, value| {
                Some(low.map_or(value, |low| low.min(value)))
            });
        let who = self.audience(cards, &runs);
        Ok(proposal_card(
            &details,
            &by_id,
            self.settings.zone,
            Some(&waiting),
            confidence,
            Some(&who),
        ))
    }

    /// Claim, send and bind one journalled post under its own lease.
    pub(super) async fn execute(
        &self,
        intent: NotificationIntent,
        message: &OutgoingMessage,
        now: DateTime<Utc>,
    ) -> Option<Result<SendOutcome, SendFailure>> {
        let lease = match self
            .store
            .begin_lease(&self.settings.instance_id, intent.effect.as_str(), now)
            .await
        {
            Ok(lease) => lease,
            Err(error) => {
                self.raise(
                    AdminAlert::JournalFailure {
                        attempt: None,
                        effect: intent.effect.clone(),
                        channel_id: intent.channel_id.clone(),
                        detail: error.to_string(),
                    },
                    now,
                );
                return None;
            }
        };
        let executor = Executor {
            journal: &*self.store,
            transport: &*self.transport,
            alerts: &*self.alerts,
            throttle: &self.throttle,
            lease: &lease,
        };
        let send = PlannedSend {
            intent,
            disposition: SendDisposition::Send,
        };
        let outcome = executor.execute(&send, message, None, None, now).await;
        // Best effort: an unended lease is orphaned by restart recovery.
        let _ = self.store.end_lease(&lease, now).await;
        Some(outcome)
    }

    async fn send_cards(
        &self,
        channel_id: &str,
        cards: &[StoredCard],
        now: DateTime<Utc>,
    ) -> PostResult {
        let view = match self.view(cards).await {
            Ok(view) => view,
            Err(_) => return PostResult::NotPosted,
        };
        let message = OutgoingMessage {
            content: Some(view.content.clone()),
            embeds: vec![embed(&view)],
            allowed_mentions: mentions::allow_users(&view.mention_users),
            reply_to: None,
            attachments: Vec::new(),
        };
        let intent = NotificationIntent {
            effect: EffectKind::Card,
            effect_context: Vec::new(),
            channel_id: channel_id.to_owned(),
            targets: cards
                .iter()
                .map(|card| DeliveryTarget::Card(card.proposal_id.clone()))
                .collect(),
            mentions: view.mention_users.clone(),
            content: IntentContent::ProposalCard {
                proposal_ids: cards.iter().map(|card| card.proposal_id.clone()).collect(),
            },
            warnings: Vec::new(),
        };
        posted(self.execute(intent, &message, now).await)
    }

    /// Save the new proposals' card details and post one card for them (after
    /// any stranded card in the channel, as v4), then mark the retired
    /// proposals' cards superseded.
    pub async fn post_card(&self, card: &crate::extract::pipeline::Card) -> PostResult {
        let now = self.now();
        let channel = &card.channel_id;
        let mut saved = true;
        for entry in &card.entries {
            let details = self.details_of(entry).await;
            if self
                .store
                .save_card(&entry.proposal_id, channel, &details, now)
                .await
                .is_err()
            {
                saved = false;
            }
        }
        let ids: Vec<String> = card.entries.iter().map(|e| e.proposal_id.clone()).collect();
        self.repost_stranded(channel, &ids, now).await;
        let result = if ids.is_empty() {
            PostResult::Posted
        } else if !saved {
            PostResult::NotPosted
        } else {
            match self.store.load_cards(&ids).await {
                // Saved details (link included) are reposted by a later pass.
                Ok(cards) if !cards.is_empty() => {
                    match self.send_cards(channel, &cards, now).await {
                        PostResult::NotPosted => PostResult::Pending,
                        result => result,
                    }
                }
                _ => PostResult::NotPosted,
            }
        };
        self.refresh_proposals(&card.superseded).await;
        result
    }

    /// v4 `_repost_stranded`: live, unexpired cards in the channel never
    /// posted, one message each so a held (maybe posted) or unavailable card
    /// never blocks another; held ones are skipped outright.
    async fn repost_stranded(&self, channel: &str, fresh: &[String], now: DateTime<Utc>) {
        let Ok(stranded) = self.store.unposted_cards(channel).await else {
            return;
        };
        let Ok(view) = self.store.load_view().await else {
            return;
        };
        for card in stranded {
            if fresh.contains(&card.proposal_id)
                || view.holds(&DeliveryTarget::Card(card.proposal_id.clone()))
            {
                continue;
            }
            let expired = match self.store.load_proposal(&card.proposal_id).await {
                Ok(Some((_, info))) => info.expires_at <= now,
                _ => true,
            };
            if expired {
                continue;
            }
            self.send_cards(channel, std::slice::from_ref(&card), now)
                .await;
        }
    }

    /// Re-render the cards these proposals are on.
    pub async fn refresh_proposals(&self, proposal_ids: &[String]) {
        let Ok(cards) = self.store.load_cards(proposal_ids).await else {
            return;
        };
        let mut messages: Vec<String> = cards
            .into_iter()
            .filter_map(|card| card.message_id)
            .collect();
        messages.sort();
        messages.dedup();
        for message in messages {
            self.refresh(&message).await;
        }
    }

    /// Re-render one posted card from its details, with a line per decision
    /// taken on it (v4's appended "applied by" / "rejected by" / superseded).
    /// `false` when it could not be edited.
    pub async fn refresh(&self, message_id: &str) -> bool {
        self.refresh_with(message_id, &[]).await
    }

    /// [`Self::refresh`] with extra lines after the decisions (the stale
    /// note after a refused ✅).
    pub async fn refresh_with(&self, message_id: &str, extra: &[&str]) -> bool {
        let Ok(cards) = self.store.cards_on_message(message_id).await else {
            return false;
        };
        let Some(first) = cards.first() else {
            return false;
        };
        let (Some(channel), Some(message)) = (parse_id(&first.channel_id), parse_id(message_id))
        else {
            return false;
        };
        let mut notices: Vec<String> = Vec::new();
        for card in &cards {
            let Ok(Some((loaded, _))) = self.store.load_proposal(&card.proposal_id).await else {
                continue;
            };
            let draft = &loaded.draft;
            let notice = match draft.status {
                DraftStatus::Merged => applied_notice(&self.actor_name(draft.closed_by.as_ref())),
                DraftStatus::Rejected => {
                    rejected_notice(&self.actor_name(draft.closed_by.as_ref()))
                }
                DraftStatus::Discarded if draft.close_reason.as_deref() == Some(SUPERSEDED) => {
                    SUPERSEDED_NOTICE.to_owned()
                }
                _ => continue,
            };
            if !notices.contains(&notice) {
                notices.push(notice);
            }
        }
        for line in extra {
            if !notices.iter().any(|notice| notice == line) {
                notices.push((*line).to_owned());
            }
        }
        let Ok(view) = self.view(&cards).await else {
            return false;
        };
        let edit = MessageEdit {
            content: Some(with_notices(&view.content, &notices)),
            embeds: Some(vec![embed(&view)]),
            allowed_mentions: mentions::none(),
        };
        self.transport
            .edit_message(channel, message, &edit)
            .await
            .is_delivered()
    }

    /// A journalled plain message with no mentions (a self-service link, an
    /// approval problem); each is its own operation.
    pub async fn post_plain(&self, channel_id: &str, kind: &str, content: String) -> PostResult {
        let now = self.now();
        let message = OutgoingMessage {
            content: Some(content),
            embeds: Vec::new(),
            allowed_mentions: mentions::none(),
            reply_to: None,
            attachments: Vec::new(),
        };
        let intent = NotificationIntent {
            effect: EffectKind::Notice(kind.to_owned()),
            effect_context: Vec::new(),
            channel_id: channel_id.to_owned(),
            targets: Vec::new(),
            mentions: Vec::new(),
            content: IntentContent::Plain,
            warnings: Vec::new(),
        };
        posted(self.execute(intent, &message, now).await)
    }

    /// A link-first self-service link, addressed to its author by name.
    pub async fn redirect(&self, redirected: &Redirected) -> PostResult {
        let tip = &redirected.tip;
        let line = render_tip(tip.lead_in.as_deref(), tip.link.purpose, &tip.link.url);
        self.post_plain(
            &redirected.channel_id,
            "notice.self_service.link",
            format!("<@{}> {line}", redirected.author_id),
        )
        .await
    }
}
