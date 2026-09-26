//! `/debug ping` and `/debug clear_test` (v4 `DebugGroup.ping`/`clear_test`,
//! `effects.delete_debug_message`): the [`DebugCards`] port.
//!
//! A ping posts the run's real card of that kind, prefixed `🧪 TEST — `, in
//! the run's home channel (else the post channel, as v4 `post_channel`). It
//! is journalled as a `debug_card` effect under its own operation (v4
//! `DedupePolicy.operation`: every ping is a new post, a retried operation
//! is not), gets ✅/❌, and on bind is registered for the run so reactions
//! drive the run's RSVPs. It never touches the run's reminder rows. People
//! follow v4's `test` audience: named, pinged only at ping level `all`.
//! Day-of test cards use v4's heading (no persona rewrite). Day-of and
//! countdown test cards are refreshed like real ones (v4 `_rebuild_test_card`).
//! `clear_test` deletes the channel's test cards of the last 24 h and marks
//! them cleared.

mod store;
mod text;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use chrono::{DateTime, Utc};

pub use store::{DebugCardStore, PostedDebugCard};

use super::alerts::{AlertThrottle, LogAlerts};
use super::cards::{self, CardContext, CardKit, fetch_art};
use super::executor::{Executor, SendOutcome};
use super::notice_text::QUIET_NOTE;
use super::refresh::Now;
use crate::bot::commands::{DebugCards, PortFuture, TestKind, TestPosted};
use crate::bot::ids::parse_id;
use crate::bot::mentions;
use crate::bot::transport::{DiscordTransport, Outcome, OutgoingMessage, RejectionKind};
use crate::domain::members::Directory;
use crate::domain::notify::{
    ChannelChoice, ChannelDirectory, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent,
    NotificationIntent, PingKind, PlannedSend, SendDisposition, choose_channel, resolve_mentions,
};
use crate::domain::schedule::{Run, SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};

/// v4 `TEST_PREFIX`.
pub const TEST_PREFIX: &str = "🧪 TEST — ";

/// v4's `test` audience: the party, pinged only if they asked for all pings;
/// nobody while quiet.
pub fn test_mentions(members: &dyn Directory, run: &Run, quiet: bool) -> Vec<String> {
    if quiet {
        return Vec::new();
    }
    let mut users = resolve_mentions(members, &run.participants, &PingKind::Test);
    users.sort();
    users.dedup();
    users
}

pub struct DebugDesk<S, T> {
    pub store: Arc<S>,
    pub transport: Arc<T>,
    pub members: Arc<dyn Directory + Send + Sync>,
    pub channels: Arc<dyn ChannelDirectory + Send + Sync>,
    pub cards: CardKit,
    pub policy: SchedulePolicy,
    /// The tick's live quiet-mode and post-channel settings.
    pub quiet: Arc<AtomicBool>,
    pub post_channel: Arc<RwLock<Option<String>>>,
    pub instance_id: String,
    pub now: Now,
    pub throttle: AlertThrottle,
}

impl<S, T> DebugDesk<S, T>
where
    S: ScheduleStore + DeliveryJournal + DebugCardStore + Send + Sync,
    T: DiscordTransport,
{
    async fn post_test(
        &self,
        run_id: &str,
        kind: TestKind,
        requested_by: &str,
    ) -> Result<TestPosted, String> {
        let now = (self.now)();
        let schedule = self
            .store
            .load(&Scope::All)
            .await
            .map_err(|error| format!("store: {error}"))?;
        let run = schedule
            .runs
            .iter()
            .find(|run| run.id == run_id)
            .ok_or_else(|| format!("run {run_id} vanished"))?;
        let post = self
            .post_channel
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let channel_id =
            match choose_channel(run.channel_id.as_deref(), post.as_deref(), &*self.channels) {
                ChannelChoice::Requested(channel)
                | ChannelChoice::Fallback {
                    channel_id: channel,
                    ..
                } => channel,
                ChannelChoice::Unavailable => return Ok(TestPosted::Unreachable),
            };
        let quiet = self.quiet.load(Ordering::Relaxed);
        let mentioned = test_mentions(&*self.members, run, quiet);
        let message = self
            .render(&schedule, run, kind, &mentioned, requested_by, quiet)
            .await?;
        let intent = NotificationIntent {
            effect: EffectKind::DebugCard,
            effect_context: vec![run.id.clone(), kind.as_str().to_owned()],
            channel_id: channel_id.clone(),
            targets: vec![DeliveryTarget::DebugCard {
                run_id: run.id.clone(),
                kind: kind.as_str().to_owned(),
            }],
            mentions: mentioned,
            content: IntentContent::Plain,
            warnings: Vec::new(),
        };
        let lease = self
            .store
            .begin_lease(&self.instance_id, EffectKind::DebugCard.as_str(), now)
            .await
            .map_err(|error| format!("journal: {error}"))?;
        let executor = Executor {
            journal: &*self.store,
            transport: &*self.transport,
            alerts: &LogAlerts,
            throttle: &self.throttle,
            lease: &lease,
        };
        let send = PlannedSend {
            intent,
            disposition: SendDisposition::Send,
        };
        let outcome = executor.execute(&send, &message, None, None, now).await;
        // Best effort: an unended lease is orphaned by restart recovery.
        let _ = self.store.end_lease(&lease, now).await;
        match outcome {
            Ok(SendOutcome::Bound(_)) => Ok(TestPosted::Posted { channel_id }),
            Ok(SendOutcome::Unavailable(detail)) => Err(detail),
            // Maybe posted, refused or never sent: say so, as v4 did.
            Ok(_) => Ok(TestPosted::Unconfirmed),
            // Discord took it but the journal write after failed: it may be up.
            Err(failure) if failure.maybe_delivered => Ok(TestPosted::Unconfirmed),
            Err(failure) => Err(format!("journal: {}", failure.error)),
        }
    }

    /// The real message of `kind` for `run`, prefixed; plain texts carry
    /// v4's quiet line while quiet (cards never announce it).
    async fn render(
        &self,
        schedule: &ScheduleSnapshot,
        run: &Run,
        kind: TestKind,
        mentioned: &[String],
        requested_by: &str,
        quiet: bool,
    ) -> Result<OutgoingMessage, String> {
        let ctx = CardContext {
            schedule,
            attendance: self.policy.attendance,
            zone: self.policy.zone(),
            quiet,
            members: &*self.members,
            catalog: self.cards.catalog.as_deref(),
        };
        let card_content = match kind {
            TestKind::DayOf => Some(IntentContent::DayOf {
                run_ids: vec![run.id.clone()],
            }),
            TestKind::Countdown60 | TestKind::Countdown15 => Some(IntentContent::Countdown {
                run_id: run.id.clone(),
                minutes: if kind == TestKind::Countdown60 {
                    60
                } else {
                    15
                },
            }),
            TestKind::Amend | TestKind::Decline => None,
        };
        if let Some(content) = card_content {
            let card = cards::build(&content, &ctx, None, mentioned)
                .ok_or_else(|| format!("no {} card for run {}", kind.as_str(), run.id))?;
            let card = cards::Card {
                content: format!("{TEST_PREFIX}{}", card.content),
                ..card
            };
            let pictures = fetch_art(self.cards.art.as_ref(), &card, true).await;
            return Ok(card.message(mentioned, &pictures));
        }
        let text = if kind == TestKind::Decline {
            let name = self.members.display_name(requested_by).unwrap_or_else(|| {
                if quiet {
                    cards::UNNAMED.to_owned()
                } else {
                    format!("<@{requested_by}>")
                }
            });
            text::decline_text(&ctx, run, mentioned, requested_by, &name)
        } else {
            text::amend_text(&ctx, run, mentioned)
        };
        let mut content = format!("{TEST_PREFIX}{text}");
        if quiet {
            // v4 `formatting.quieted` for a message without an embed.
            content = format!("{content}\n_{QUIET_NOTE}_");
        }
        Ok(OutgoingMessage {
            content: Some(content),
            embeds: Vec::new(),
            allowed_mentions: mentions::allow_users(mentioned),
            reply_to: None,
            attachments: Vec::new(),
        })
    }

    /// Delete each test card in `channel_id` since `since`; a message
    /// already gone counts as deleted. `(deleted, failed)`.
    async fn clear_tests(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> Result<(usize, usize), String> {
        let posted = self
            .store
            .debug_cards_in(channel_id, since)
            .await
            .map_err(|error| format!("store: {error}"))?;
        let (mut deleted, mut failed) = (0, 0);
        for card in posted {
            let (Some(channel), Some(message)) =
                (parse_id(&card.channel_id), parse_id(&card.message_id))
            else {
                failed += 1;
                continue;
            };
            let gone = matches!(
                self.transport.delete_message(channel, message).await,
                Outcome::Delivered(()) | Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
            );
            if gone
                && self
                    .store
                    .clear_debug_card(&card.message_id, (self.now)())
                    .await
                    .is_ok()
            {
                deleted += 1;
            } else {
                failed += 1;
            }
        }
        Ok((deleted, failed))
    }
}

impl<S, T> DebugCards for DebugDesk<S, T>
where
    S: ScheduleStore + DeliveryJournal + DebugCardStore + Send + Sync + 'static,
    T: DiscordTransport + 'static,
{
    fn post(
        &self,
        run_id: String,
        kind: TestKind,
        requested_by: String,
    ) -> PortFuture<'_, Result<TestPosted, String>> {
        Box::pin(async move { self.post_test(&run_id, kind, &requested_by).await })
    }

    fn clear(
        &self,
        channel_id: String,
        since: DateTime<Utc>,
    ) -> PortFuture<'_, Result<(usize, usize), String>> {
        Box::pin(async move { self.clear_tests(&channel_id, since).await })
    }
}
