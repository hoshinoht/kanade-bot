//! Delivery and confirmed retraction of durable decline notices.

use chrono::{DateTime, Utc};

use super::alerts::AlertSink;
use super::executor::SendReport;
use super::tick::{Delivery, DeliveryError, TickLease};
use crate::app::ids::parse_id;
use crate::cards::CardKit;
use crate::cards::redesign::decline_text;
use crate::discord::mentions::allow_users;
use crate::discord::transport::{DiscordTransport, Outcome, OutgoingMessage, RejectionKind};
use kanade_domain::drafts::ProposalStore;
use kanade_domain::history::Checkpoints;
use kanade_domain::members::Directory;
use kanade_domain::notify::{
    DeclineNotice, DeclineNoticeStore, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent,
    NotificationIntent, PlannedSend, SendDisposition, audience,
};
use kanade_domain::notify::{PingKind, ReminderCardStore};
use kanade_domain::scheduler::{IdSource, ScheduleStore, Scope};

/// What the recovery drain did for decline notices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeclineReport {
    pub sends: Vec<SendReport>,
    pub retracted: usize,
}

/// The text and the users it may ping: it leads with the decliner and
/// tags the rest of the party in a `for …` subtext line.
fn render_notice(
    notice: &DeclineNotice,
    run: &kanade_domain::schedule::Run,
    members: &dyn Directory,
    kit: &CardKit,
) -> (String, Vec<String>) {
    let others: Vec<String> = run
        .participants
        .iter()
        .filter(|user| *user != &notice.user_id)
        .cloned()
        .collect();
    let audience = audience(members, &others, &PingKind::Decline, None);
    let names: std::collections::BTreeMap<_, _> = audience.names.iter().cloned().collect();
    let people = others
        .iter()
        .map(|user| {
            if audience.mentioned.contains(user) {
                format!("<@{user}>")
            } else {
                names
                    .get(user)
                    .cloned()
                    .unwrap_or_else(|| format!("<@{user}>"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    let display_name = notice.display_name.as_deref().unwrap_or_default();
    let decliner = if display_name.is_empty() {
        format!("<@{}>", notice.user_id)
    } else {
        display_name.to_owned()
    };
    let text = decline_text(&decliner, run, &people, kit.catalog.as_deref(), &kit.marks);
    (text, audience.mentioned)
}

impl<S, I, T, A> Delivery<'_, S, I, T, A>
where
    S: ScheduleStore
        + DeliveryJournal
        + DeclineNoticeStore
        + kanade_domain::notify::NoticeOutbox
        + Checkpoints
        + ProposalStore
        + ReminderCardStore
        + Sync,
    I: IdSource,
    T: DiscordTransport,
    A: AlertSink,
{
    /// Drain durable candidates after startup and on later recovery ticks.
    pub async fn drain_decline_notices(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<DeclineReport, DeliveryError> {
        self.leased(now, async move |this: &mut Self, lease: &TickLease| {
            this.declines_in(lease, now).await
        })
        .await
    }

    async fn declines_in(
        &self,
        lease: &TickLease,
        now: DateTime<Utc>,
    ) -> Result<DeclineReport, DeliveryError> {
        let rows = self
            .store
            .pending_decline_notices(kanade_domain::notify::MAX_PENDING_DECLINE_NOTICES)
            .await?;
        let snapshot = self.store.load(&Scope::All).await?;
        let mut report = DeclineReport::default();
        for notice in rows {
            if notice.message_id.is_some() {
                if notice.retract_pending && self.retract_decline_in(lease, &notice, now).await? {
                    report.retracted += 1;
                }
                continue;
            }
            // A retraction that won before create must never create a replacement.
            if notice.retract_pending {
                continue;
            }
            let Some(channel_id) = notice.channel_id.as_deref() else {
                continue;
            };
            let Some(run) = snapshot.runs.iter().find(|run| run.id == notice.run_id) else {
                continue;
            };
            let (content, mentions) = render_notice(&notice, run, self.members, &self.cards);
            let intent = NotificationIntent {
                effect: EffectKind::Notice("decline.notice".into()),
                effect_context: vec![notice.run_id.clone(), notice.user_id.clone()],
                channel_id: channel_id.to_owned(),
                targets: vec![DeliveryTarget::Decline {
                    run_id: notice.run_id.clone(),
                    user_id: notice.user_id.clone(),
                }],
                mentions,
                content: IntentContent::Plain,
                warnings: Vec::new(),
            };
            let message = OutgoingMessage {
                content: Some(content),
                embeds: Vec::new(),
                allowed_mentions: allow_users(&intent.mentions),
                reply_to: notice.reference_id.as_deref().and_then(parse_id),
                attachments: Vec::new(),
                components: Vec::new(),
            };
            let send = PlannedSend {
                intent: intent.clone(),
                disposition: SendDisposition::Send,
            };
            let outcome = super::tick::settle(
                self.executor(self.lease(lease).await?)
                    .execute(&send, &message, None, None, now)
                    .await,
            )?;
            report.sends.push(SendReport { intent, outcome });
            let bound = self
                .store
                .decline_notice(&notice.run_id, &notice.user_id)
                .await?;
            if let Some(bound) = bound.filter(|row| row.retract_pending) {
                if bound.message_id.is_some() {
                    if self.retract_decline_in(lease, &bound, now).await? {
                        report.retracted += 1;
                    }
                } else {
                    let lease = self.lease(lease).await?;
                    self.store
                        .resolve_decline_retract_pending(lease, &bound.run_id, &bound.user_id, now)
                        .await?;
                }
            }
        }
        Ok(report)
    }

    /// S3 entry point: mark an in-flight candidate pending, otherwise delete
    /// and atomically retire the exact bound attempt.
    pub async fn retract_decline_notice(
        &mut self,
        run_id: &str,
        user_id: &str,
        now: DateTime<Utc>,
    ) -> Result<bool, DeliveryError> {
        let lease = TickLease::new(now);
        let result = async {
            let Some(notice) = self.store.decline_notice(run_id, user_id).await? else {
                return Ok(false);
            };
            if notice.message_id.is_none() {
                return self
                    .store
                    .mark_decline_retract_pending(run_id, user_id)
                    .await
                    .map_err(DeliveryError::from);
            }
            self.retract_decline_in(&lease, &notice, now).await
        }
        .await;
        self.finish(lease, result).await
    }

    async fn retract_decline_in(
        &self,
        lease: &TickLease,
        notice: &DeclineNotice,
        now: DateTime<Utc>,
    ) -> Result<bool, DeliveryError> {
        let (Some(channel_id), Some(message_id)) =
            (notice.channel_id.as_deref(), notice.message_id.as_deref())
        else {
            return Ok(false);
        };
        let (Some(channel), Some(message)) = (parse_id(channel_id), parse_id(message_id)) else {
            return Ok(false);
        };
        let deleted = match self.transport.delete_message(channel, message).await {
            Outcome::Delivered(()) | Outcome::DefinitelyRejected(RejectionKind::UnknownMessage) => {
                true
            }
            Outcome::Ambiguous(_) => matches!(
                self.transport.delete_message(channel, message).await,
                Outcome::Delivered(()) | Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
            ),
            Outcome::DefinitelyRejected(_) => false,
        };
        if !deleted {
            return Ok(false);
        }
        let lease = self.lease(lease).await?;
        self.store
            .retire_decline_retraction(
                lease,
                &notice.run_id,
                &notice.user_id,
                channel_id,
                message_id,
                now,
            )
            .await?;
        Ok(true)
    }
}
