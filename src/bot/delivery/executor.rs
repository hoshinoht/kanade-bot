//! One planned send: claim, transport, then bind or resolve (v4
//! `DeliveryJournal.execute`, with v5's retry-safe and rejection outcomes).
//!
//! | Transport outcome | Journal |
//! | --- | --- |
//! | `Delivered` | `bind` (the channel sent to, `record_week` for digests) |
//! | `Ambiguous`, or `bind` failed | `mark_indeterminate`: held, never resent |
//! | `NotSent`, `RateLimited` | `release_unsent`: claimable again next tick |
//! | any other definite rejection | `retire_rejected` and an admin alert |
//!
//! A journal write that fails after a fresh claim raises
//! [`AdminAlert::JournalFailure`] naming the attempt. The intent stays held
//! until restart recovery makes it indeterminate; it is never resent.

use chrono::{DateTime, Utc};

use super::alerts::{AdminAlert, AlertSink, AlertThrottle};
use crate::bot::ids::parse_id;
use crate::bot::transport::{
    DiscordTransport, MessageId, Outcome, OutgoingMessage, Presence, RejectionKind,
};
use crate::domain::notify::{
    AttemptId, Claim, DeliveryJournal, DeliveryWarning, EffectKind, JournalError, Lease,
    NotificationIntent, PlannedSend, Receipt, SendDisposition, WeeklyDigest,
};
use crate::domain::schedule::{EMOJI_NO, EMOJI_YES};

/// What happened to one planned send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// Posted and bound.
    Bound(MessageId),
    /// Maybe posted; held forever, never resent.
    Uncertain,
    /// An earlier unresolved attempt holds it (at planning or at claim).
    Suppressed,
    /// A planned row vanished or was sent before the claim; nothing sent.
    Unavailable(String),
    /// Never reached Discord; released for the next tick.
    Released(RejectionKind),
    /// Discord refused it; retired, alerted, not retried.
    Rejected(RejectionKind),
    /// A journal write failed; alerted, and the tick moved on.
    Failed(String),
}

impl SendOutcome {
    /// Whether the send was claimed (and so counts against the tick's cap).
    pub fn claimed(&self) -> bool {
        matches!(
            self,
            Self::Bound(_) | Self::Uncertain | Self::Released(_) | Self::Rejected(_)
        )
    }
}

/// A journal failure for one send. `attempt` is set once a claim succeeded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendFailure {
    pub attempt: Option<AttemptId>,
    pub error: JournalError,
    /// Discord accepted the post or its outcome is unknown (the journal
    /// write after it failed): it may be visible.
    pub maybe_delivered: bool,
}

impl SendFailure {
    /// Lease loss or an unavailable backend stops the whole tick; anything
    /// else only fails this send.
    pub fn aborts(&self) -> bool {
        matches!(
            self.error,
            JournalError::LeaseNotLive | JournalError::Backend(_)
        )
    }
}

/// A send and its outcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendReport {
    pub intent: NotificationIntent,
    pub outcome: SendOutcome,
}

/// The result of trying to replace a week's digest card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Replacement {
    /// The old card is confirmed gone and its claim retired.
    Retired,
    /// Deletion was not confirmed; the new card must not be posted.
    Suppressed(String),
}

/// Runs sends under one live lease.
pub struct Executor<'a, J, T, A> {
    pub journal: &'a J,
    pub transport: &'a T,
    pub alerts: &'a A,
    pub throttle: &'a AlertThrottle,
    pub lease: &'a Lease,
}

/// Released, not retried now: the send never reached Discord.
fn retryable(kind: &RejectionKind) -> bool {
    matches!(kind, RejectionKind::NotSent | RejectionKind::RateLimited)
}

fn reason_text(kind: &RejectionKind) -> String {
    format!("discord rejected the post: {kind:?}")
}

impl<J, T, A> Executor<'_, J, T, A>
where
    J: DeliveryJournal + Sync,
    T: DiscordTransport,
    A: AlertSink,
{
    /// Pass `alert` on unless an identical one went out within the window.
    pub fn raise(&self, alert: AdminAlert, now: DateTime<Utc>) {
        if self.throttle.admit(&alert, now) {
            self.alerts.alert(alert);
        }
    }

    /// Execute one planned send. A notice passes its operation's effect
    /// ordinal; native sends pass `None`.
    ///
    /// # Errors
    /// Journal failures other than a vanished target, already alerted. A
    /// failure after the claim leaves the attempt held, never resendable.
    pub async fn execute(
        &self,
        send: &PlannedSend,
        message: &OutgoingMessage,
        effect_ordinal: Option<i64>,
        record_week: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<SendOutcome, SendFailure> {
        if send.disposition == SendDisposition::Suppressed {
            return Ok(SendOutcome::Suppressed);
        }
        let intent = &send.intent;
        let claim = self
            .journal
            .claim(self.lease, intent, effect_ordinal, now)
            .await;
        self.claimed(intent, claim, message, record_week, now).await
    }

    /// Execute one outbox notice, claimed by its durable `(source, ordinal)`
    /// key ([`DeliveryJournal::claim_source`]) so no later lease resends it.
    ///
    /// # Errors
    /// As [`Executor::execute`].
    pub async fn execute_source(
        &self,
        intent: &NotificationIntent,
        message: &OutgoingMessage,
        source: &str,
        ordinal: i64,
        now: DateTime<Utc>,
    ) -> Result<SendOutcome, SendFailure> {
        let claim = self
            .journal
            .claim_source(self.lease, intent, source, ordinal, now)
            .await;
        self.claimed(intent, claim, message, None, now).await
    }

    async fn claimed(
        &self,
        intent: &NotificationIntent,
        claim: Result<Claim, JournalError>,
        message: &OutgoingMessage,
        record_week: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<SendOutcome, SendFailure> {
        let attempt = match claim {
            Ok(Claim::Fresh(attempt)) => attempt,
            Ok(Claim::Held) => return Ok(SendOutcome::Suppressed),
            Err(JournalError::TargetUnavailable(detail)) => {
                return Ok(SendOutcome::Unavailable(detail));
            }
            Err(error) => return Err(self.failed(intent, None, error, now)),
        };
        self.warn(intent, now);
        self.deliver(intent, &attempt, message, record_week, now)
            .await
            .map_err(|(error, maybe_delivered)| {
                let mut failure = self.failed(intent, Some(attempt), error, now);
                failure.maybe_delivered = maybe_delivered;
                failure
            })
    }

    fn failed(
        &self,
        intent: &NotificationIntent,
        attempt: Option<AttemptId>,
        error: JournalError,
        now: DateTime<Utc>,
    ) -> SendFailure {
        self.raise(
            AdminAlert::JournalFailure {
                attempt: attempt.clone(),
                effect: intent.effect.clone(),
                channel_id: intent.channel_id.clone(),
                detail: error.to_string(),
            },
            now,
        );
        SendFailure {
            attempt,
            error,
            maybe_delivered: false,
        }
    }

    async fn deliver(
        &self,
        intent: &NotificationIntent,
        attempt: &AttemptId,
        message: &OutgoingMessage,
        record_week: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<SendOutcome, (JournalError, bool)> {
        let outcome = match parse_id(&intent.channel_id) {
            Some(channel) => self.transport.create_message(channel, message).await,
            None => Outcome::DefinitelyRejected(RejectionKind::Invalid),
        };
        let maybe = |error| (error, true);
        let refused = |error| (error, false);
        match outcome {
            Outcome::Delivered(message_id) => self
                .bind(intent, attempt, message_id, record_week, now)
                .await
                .map_err(maybe),
            Outcome::Ambiguous(_) => {
                self.journal
                    .mark_indeterminate(self.lease, attempt)
                    .await
                    .map_err(maybe)?;
                Ok(SendOutcome::Uncertain)
            }
            Outcome::DefinitelyRejected(kind) if retryable(&kind) => {
                self.journal
                    .release_unsent(self.lease, attempt, &reason_text(&kind), now)
                    .await
                    .map_err(refused)?;
                Ok(SendOutcome::Released(kind))
            }
            Outcome::DefinitelyRejected(kind) => {
                self.journal
                    .retire_rejected(self.lease, attempt, &reason_text(&kind), now)
                    .await
                    .map_err(refused)?;
                self.raise(
                    AdminAlert::SendRejected {
                        effect: intent.effect.clone(),
                        channel_id: intent.channel_id.clone(),
                        reason: kind.clone(),
                    },
                    now,
                );
                Ok(SendOutcome::Rejected(kind))
            }
        }
    }

    fn warn(&self, intent: &NotificationIntent, now: DateTime<Utc>) {
        for warning in &intent.warnings {
            let DeliveryWarning::HomeChannelUnavailable {
                home_channel_id,
                run_ids,
            } = warning;
            self.raise(
                AdminAlert::HomeChannelUnavailable {
                    home_channel_id: home_channel_id.clone(),
                    used_channel_id: intent.channel_id.clone(),
                    run_ids: run_ids.clone(),
                },
                now,
            );
        }
    }

    async fn bind(
        &self,
        intent: &NotificationIntent,
        attempt: &AttemptId,
        message_id: MessageId,
        record_week: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> Result<SendOutcome, JournalError> {
        let receipt = Receipt {
            channel_id: intent.channel_id.clone(),
            message_id: message_id.get().to_string(),
        };
        if self
            .journal
            .bind(self.lease, attempt, &receipt, record_week, now)
            .await
            .is_err()
        {
            self.journal.mark_indeterminate(self.lease, attempt).await?;
            return Ok(SendOutcome::Uncertain);
        }
        if matches!(intent.effect, EffectKind::Reminder | EffectKind::Card)
            && let Some(channel) = parse_id(&intent.channel_id)
        {
            // Best effort, as v4: a card without reactions still counts.
            for emoji in [EMOJI_YES, EMOJI_NO] {
                let _ = self
                    .transport
                    .add_own_reaction(channel, message_id, emoji)
                    .await;
            }
        }
        Ok(SendOutcome::Bound(message_id))
    }

    /// Replace `old` only once Discord confirms it is gone: deleted now,
    /// already Unknown Message, or absent on a follow-up fetch. Anything
    /// else suppresses the replacement and alerts.
    ///
    /// # Errors
    /// A lost lease or backend failure while retiring the old claim.
    pub async fn replace_digest(
        &self,
        old: &WeeklyDigest,
        now: DateTime<Utc>,
    ) -> Result<Replacement, JournalError> {
        let replacement = match self.confirm_deleted(old).await {
            Ok(()) => match self
                .journal
                .retire_for_replacement(self.lease, old, now)
                .await
            {
                Ok(()) => Replacement::Retired,
                Err(error @ (JournalError::LeaseNotLive | JournalError::Backend(_))) => {
                    return Err(error);
                }
                Err(error) => Replacement::Suppressed(error.to_string()),
            },
            Err(reason) => Replacement::Suppressed(reason),
        };
        if let Replacement::Suppressed(reason) = &replacement {
            self.raise(
                AdminAlert::DigestReplacementSuppressed {
                    week_start: old.week_start,
                    channel_id: old.channel_id.clone(),
                    message_id: old.message_id.clone(),
                    reason: reason.clone(),
                },
                now,
            );
        }
        Ok(replacement)
    }

    async fn confirm_deleted(&self, old: &WeeklyDigest) -> Result<(), String> {
        let (Some(channel), Some(message)) = (parse_id(&old.channel_id), parse_id(&old.message_id))
        else {
            return Err("the stored card has no valid Discord ids".into());
        };
        match self.transport.delete_message(channel, message).await {
            Outcome::Delivered(()) | Outcome::DefinitelyRejected(RejectionKind::UnknownMessage) => {
                Ok(())
            }
            Outcome::DefinitelyRejected(kind) => Err(format!("deletion refused: {kind:?}")),
            Outcome::Ambiguous(kind) => {
                match self.transport.message_presence(channel, message).await {
                    Outcome::Delivered(Presence::Absent) => Ok(()),
                    _ => Err(format!("deletion unconfirmed: {kind:?}")),
                }
            }
        }
    }
}
