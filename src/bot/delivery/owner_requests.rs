//! Weekly-timing ownership requests on Discord (user decision 2026-10-10):
//! each tick expires open requests past their 24 hours, posts every open
//! request not yet posted in its timing's channel (pinging the owner, with
//! Accept/Decline buttons), and edits each decided request's post to show
//! the decision without buttons.
//!
//! The post is a target-less journalled effect claimed by the durable source
//! key `owner-request:<id>`, so a crash or an ambiguous send never posts it
//! twice. A post is legacy text with a button row (a V2 ping arrives as an
//! empty push notification). Edits are idempotent and retried until Discord
//! confirms them or the message is gone.

use chrono::{DateTime, Utc};
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::ButtonStyle;

use super::alerts::{AdminAlert, AlertSink};
use super::cards::redesign::{ButtonId, action_row, button};
use super::executor::SendOutcome;
use super::tick::{Delivery, DeliveryError, settle};
use crate::bot::commands::text::{hhmm, weekday_name};
use crate::bot::ids::{id_text, parse_id};
use crate::bot::mentions;
use crate::bot::transport::{
    DiscordTransport, MessageEdit, Outcome, OutgoingMessage, RejectionKind,
};
use crate::domain::drafts::ProposalStore;
use crate::domain::history::Checkpoints;
use crate::domain::ids::short_id;
use crate::domain::notify::{
    ChannelChoice, DeliveryJournal, EffectKind, IntentContent, Lease, NoticeOutbox,
    NotificationIntent, choose_channel,
};
use crate::domain::ownership::{OwnerRequest, OwnerRequestStatus, OwnerRequestStore};
use crate::domain::schedule::FixedRun;
use crate::domain::scheduler::{IdSource, ScheduleStore, Scope, StoreError};

/// The post's journal effect kind.
pub const OWNER_REQUEST_EFFECT: &str = "notice.owner.request";

/// What one tick did with ownership requests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OwnerRequestReport {
    pub expired: Vec<String>,
    /// Request ids posted (bound) this tick.
    pub posted: Vec<String>,
    /// Request ids whose post now shows the decision.
    pub settled: Vec<String>,
}

fn timing_text(fixed: Option<&FixedRun>, fixed_id: &str) -> String {
    match fixed {
        Some(fixed) => format!(
            "weekly timing `#{}` ({} {})",
            short_id(fixed_id),
            weekday_name(fixed.weekday),
            hhmm(fixed.time),
        ),
        None => format!("weekly timing `#{}`", short_id(fixed_id)),
    }
}

/// The open request's post.
pub fn request_text(request: &OwnerRequest, owner: &str, timing: &str) -> String {
    format!(
        "👑 <@{owner}>, <@{}> asks to own {timing}. Accept or decline within 24 hours.",
        request.requester
    )
}

/// The decided request's post, buttons gone.
pub fn decided_text(request: &OwnerRequest, timing: &str) -> String {
    let who = format!("<@{}>", request.requester);
    match request.status {
        OwnerRequestStatus::Accepted => format!("👑 {who} now owns {timing}."),
        OwnerRequestStatus::Declined => format!("{who}'s request to own {timing} was declined."),
        OwnerRequestStatus::Withdrawn => format!("{who} withdrew their request to own {timing}."),
        OwnerRequestStatus::Expired => {
            format!("{who}'s request to own {timing} expired unanswered.")
        }
        OwnerRequestStatus::Superseded | OwnerRequestStatus::Open => {
            format!("{who}'s request to own {timing} is closed: its owner changed.")
        }
    }
}

fn buttons(request_id: &str) -> Vec<Component> {
    vec![action_row(vec![
        button(
            ButtonStyle::Success,
            "Accept",
            ButtonId::OwnerAccept(request_id.to_owned()).custom_id(),
            false,
        ),
        button(
            ButtonStyle::Danger,
            "Decline",
            ButtonId::OwnerDecline(request_id.to_owned()).custom_id(),
            false,
        ),
    ])]
}

impl<S, I, T, A> Delivery<'_, S, I, T, A>
where
    S: ScheduleStore
        + DeliveryJournal
        + NoticeOutbox
        + Checkpoints
        + ProposalStore
        + super::cards::ReminderCardStore
        + OwnerRequestStore
        + Sync,
    I: IdSource,
    T: DiscordTransport,
    A: AlertSink,
{
    fn owner_alert(&self, detail: String, now: DateTime<Utc>) {
        let alert = AdminAlert::OwnerRequestFailed { detail };
        if self.throttle().admit(&alert, now) {
            self.alerts.alert(alert);
        }
    }

    /// Expire, post and settle. A store failure is alerted and the next
    /// tick retries; lease loss or a journal backend failure aborts the tick.
    ///
    /// # Errors
    /// [`DeliveryError`] from the journal, as the notice drain.
    pub(super) async fn owner_requests_in(
        &self,
        lease: &Lease,
        now: DateTime<Utc>,
    ) -> Result<OwnerRequestReport, DeliveryError> {
        let mut report = OwnerRequestReport::default();
        match self.expire_owner_requests(now).await {
            Ok(expired) => report.expired = expired,
            Err(error) => self.owner_alert(format!("expiry: {error}"), now),
        }
        let open = match self.store.open_owner_requests().await {
            Ok(open) => open,
            Err(error) => {
                self.owner_alert(format!("read: {error}"), now);
                return Ok(report);
            }
        };
        let unsettled = match self.store.unsettled_owner_requests().await {
            Ok(rows) => rows,
            Err(error) => {
                self.owner_alert(format!("read: {error}"), now);
                Vec::new()
            }
        };
        let unposted: Vec<OwnerRequest> = open
            .into_iter()
            .filter(|request| request.message_id.is_none() && request.live(now))
            .collect();
        if unposted.is_empty() && unsettled.is_empty() {
            return Ok(report);
        }
        let timings = self.store.load(&Scope::All).await?.fixed_runs;
        let find = |id: &str| timings.iter().find(|fixed| fixed.id == id);
        for request in unposted {
            let Some(fixed) = find(&request.fixed_run_id) else {
                continue;
            };
            if self.post_owner_request(lease, &request, fixed, now).await? {
                report.posted.push(request.id);
            }
        }
        for request in unsettled {
            let timing = timing_text(find(&request.fixed_run_id), &request.fixed_run_id);
            if self.settle_owner_request(&request, &timing, now).await {
                report.settled.push(request.id);
            }
        }
        Ok(report)
    }

    async fn expire_owner_requests(&self, now: DateTime<Utc>) -> Result<Vec<String>, StoreError> {
        let mut expired = Vec::new();
        for request in self.store.open_owner_requests().await? {
            if now >= request.expires_at
                && self
                    .store
                    .close_owner_request(
                        &request.id,
                        OwnerRequestStatus::Expired,
                        "system:expiry",
                        now,
                    )
                    .await?
            {
                expired.push(request.id);
            }
        }
        Ok(expired)
    }

    /// Post one open request; `true` once bound.
    async fn post_owner_request(
        &self,
        lease: &Lease,
        request: &OwnerRequest,
        fixed: &FixedRun,
        now: DateTime<Utc>,
    ) -> Result<bool, DeliveryError> {
        let channel_id = match choose_channel(
            request.channel_id.as_deref(),
            self.config.post_channel_id.as_deref(),
            self.channels,
        ) {
            ChannelChoice::Requested(channel_id) | ChannelChoice::Fallback { channel_id, .. } => {
                channel_id
            }
            ChannelChoice::Unavailable => return Ok(false),
        };
        let owner = fixed.owner().to_owned();
        let pinged = if self.config.quiet_mode {
            Vec::new()
        } else {
            vec![owner.clone()]
        };
        let timing = timing_text(Some(fixed), &fixed.id);
        let message = OutgoingMessage {
            content: Some(request_text(request, &owner, &timing)),
            embeds: Vec::new(),
            allowed_mentions: mentions::allow_users(&pinged),
            reply_to: None,
            attachments: Vec::new(),
            components: buttons(&request.id),
        };
        let intent = NotificationIntent {
            effect: EffectKind::Notice(OWNER_REQUEST_EFFECT.to_owned()),
            effect_context: vec![request.id.clone()],
            channel_id: channel_id.clone(),
            targets: Vec::new(),
            mentions: pinged,
            content: IntentContent::Plain,
            warnings: Vec::new(),
        };
        let Some(mut operation) = self.admit().await else {
            return Ok(false);
        };
        let source = format!("owner-request:{}", request.id);
        let result = self
            .executor(lease)
            .execute_source_in_operation(&mut operation, &intent, &message, &source, 0, now)
            .await;
        let outcome = match result {
            Ok(Some(outcome)) => Ok(outcome),
            Ok(None) => {
                operation.settle();
                return Ok(false);
            }
            Err(failure) => Err(failure),
        };
        let outcome = match settle(outcome) {
            Ok(outcome) => outcome,
            Err(error) => {
                operation.settle();
                return Err(error);
            }
        };
        let bound = match outcome {
            SendOutcome::Bound(message_id) => {
                let stored = self
                    .store
                    .set_owner_request_message(&request.id, &channel_id, &id_text(message_id))
                    .await;
                if let Err(error) = stored {
                    self.owner_alert(format!("record post of {}: {error}", request.id), now);
                }
                true
            }
            _ => false,
        };
        operation.settle();
        Ok(bound)
    }

    /// Edit a decided request's post; `true` once it shows the decision or
    /// is gone.
    async fn settle_owner_request(
        &self,
        request: &OwnerRequest,
        timing: &str,
        now: DateTime<Utc>,
    ) -> bool {
        let (Some(channel), Some(message)) = (
            request.channel_id.as_deref().and_then(parse_id),
            request.message_id.as_deref().and_then(parse_id),
        ) else {
            return self.mark_settled(request, now).await;
        };
        let Some(mut operation) = self.admit().await else {
            return false;
        };
        if !operation.begin() {
            return false;
        }
        let edit = MessageEdit::legacy(decided_text(request, timing), Vec::new(), mentions::none());
        let outcome = self.transport.edit_message(channel, message, &edit).await;
        operation.settle();
        match outcome {
            Outcome::Delivered(())
            | Outcome::DefinitelyRejected(
                RejectionKind::UnknownMessage
                | RejectionKind::UnknownChannel
                | RejectionKind::MissingAccess
                | RejectionKind::MissingPermissions,
            ) => self.mark_settled(request, now).await,
            _ => false,
        }
    }

    async fn mark_settled(&self, request: &OwnerRequest, now: DateTime<Utc>) -> bool {
        match self.store.settle_owner_request_message(&request.id).await {
            Ok(()) => true,
            Err(error) => {
                self.owner_alert(format!("settle {}: {error}", request.id), now);
                false
            }
        }
    }
}
