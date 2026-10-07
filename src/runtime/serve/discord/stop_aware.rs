//! Serve's one Discord transport under the shutdown deadline (the tick, the
//! card desk, the manual digest, card refresh, decline retraction, commands,
//! chat, extraction, roster and identity all share it). Nothing using it is
//! cancelled (its store transactions finish); instead each Discord call ends
//! `budget::SEND_FINALISE` (1 s) before the cutoff with the outcome the transport's own
//! deadline gives: a cut write may have reached Discord and is
//! [`AmbiguousKind::Timeout`] (journalled indeterminate, never re-sent); a
//! cut read, or any call begun after that point, is `NotSent`. The second
//! left lets the caller record that outcome through its usual journal path
//! before workers are aborted and reads are refused.
//!
//! The cards' heading and phrase rewrites (model calls, made only by the
//! `HeaderPregen` worker, each capped by `PREGEN_DEADLINE`) end by the
//! cutoff through [`cards`]: a cut or late rewrite fails as `Unavailable`,
//! so nothing is stored and the send keeps its seed text, exactly as on a
//! model timeout. Nothing a tick awaits is cut: its store reads fail at once
//! past the cutoff (the store refuses them, see
//! `ShutdownClock::refusing_reads`) and its writes are atomic and short.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use twilight_model::channel::message::{MessageFlags, ReactionType};
use twilight_model::channel::{Channel, Message};
use twilight_model::guild::Member;
use twilight_model::id::{
    Id,
    marker::{ApplicationMarker, GuildMarker, UserMarker},
};
use twilight_model::user::CurrentUser;

use super::super::budget::ShutdownClock;
use super::GatewayTransport;
use crate::bot::delivery::cards::CardKit;
use crate::bot::transport::{
    AmbiguousKind, ChannelId, DiscordTransport, HistoryPage, InteractionRef, InteractionReply,
    MessageEdit, MessageId, Outcome, OutgoingMessage, Presence, RejectionKind,
};
use crate::chat::nudge::{NudgeRewriter, RewriteFailure, RewritePrompt, SharedRewriter};
use twilight_model::application::command::{Command, CommandOptionChoice};

pub struct StopAware<T> {
    inner: Arc<T>,
    clock: ShutdownClock,
}

impl<T> StopAware<T> {
    pub fn new(inner: Arc<T>, clock: ShutdownClock) -> Self {
        Self { inner, clock }
    }

    async fn race<U>(&self, write: bool, call: impl Future<Output = Outcome<U>>) -> Outcome<U> {
        let not_sent = || Outcome::DefinitelyRejected(RejectionKind::NotSent);
        let ended = self.clock.sends_ended();
        tokio::pin!(ended);
        // Begun after the cut: nothing is sent.
        if already(&mut ended) {
            return not_sent();
        }
        tokio::select! {
            biased;
            outcome = call => outcome,
            () = &mut ended => {
                self.clock.cut("discord_send");
                if write { Outcome::Ambiguous(AmbiguousKind::Timeout) } else { not_sent() }
            }
        }
    }
}

/// Whether `expired` is already resolved, without waiting.
fn already(expired: &mut std::pin::Pin<&mut impl Future<Output = ()>>) -> bool {
    let waker = std::task::Waker::noop();
    let mut context = std::task::Context::from_waker(waker);
    expired.as_mut().poll(&mut context).is_ready()
}

impl<T: GatewayTransport> GatewayTransport for StopAware<T> {
    fn application_ready(&self, application: Id<ApplicationMarker>) {
        self.inner.application_ready(application);
    }
}

/// `kit` with its rewriter (if any) ending by `clock`'s deadline.
pub fn cards(mut kit: CardKit, clock: ShutdownClock) -> CardKit {
    kit.heading.rewriter = kit
        .heading
        .rewriter
        .map(|inner| SharedRewriter(Arc::new(StopAwareRewrite { inner, clock })));
    kit
}

struct StopAwareRewrite {
    inner: SharedRewriter,
    clock: ShutdownClock,
}

impl NudgeRewriter for StopAwareRewrite {
    async fn rewrite(
        &self,
        prompt: &RewritePrompt,
        deadline: Duration,
    ) -> Result<String, RewriteFailure> {
        let expired = self.clock.expired();
        tokio::pin!(expired);
        if already(&mut expired) {
            return Err(RewriteFailure::Unavailable);
        }
        tokio::select! {
            biased;
            text = self.inner.rewrite(prompt, deadline) => text,
            () = &mut expired => {
                self.clock.cut("rewrite");
                Err(RewriteFailure::Unavailable)
            }
        }
    }
}

impl<T: DiscordTransport> DiscordTransport for StopAware<T> {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        self.race(true, self.inner.create_message(channel, message))
            .await
    }

    async fn create_flagged_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> Outcome<MessageId> {
        self.race(
            true,
            self.inner.create_flagged_message(channel, message, flags),
        )
        .await
    }

    async fn trigger_typing(&self, channel: ChannelId) -> Outcome<()> {
        self.race(true, self.inner.trigger_typing(channel)).await
    }

    async fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> Outcome<()> {
        self.race(true, self.inner.edit_message(channel, message, edit))
            .await
    }

    async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        self.race(true, self.inner.delete_message(channel, message))
            .await
    }

    async fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        self.race(true, self.inner.add_own_reaction(channel, message, emoji))
            .await
    }

    async fn reaction_users(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
        kind: ReactionType,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Id<UserMarker>>> {
        self.race(
            false,
            self.inner
                .reaction_users(channel, message, emoji, kind, after, limit),
        )
        .await
    }

    async fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        self.race(
            true,
            self.inner.remove_own_reaction(channel, message, emoji),
        )
        .await
    }

    async fn message_presence(&self, channel: ChannelId, message: MessageId) -> Outcome<Presence> {
        self.race(false, self.inner.message_presence(channel, message))
            .await
    }

    async fn respond(&self, interaction: &InteractionRef, reply: &InteractionReply) -> Outcome<()> {
        self.race(true, self.inner.respond(interaction, reply))
            .await
    }

    async fn defer(&self, interaction: &InteractionRef, ephemeral: bool) -> Outcome<()> {
        self.race(true, self.inner.defer(interaction, ephemeral))
            .await
    }

    async fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        self.race(true, self.inner.complete_deferred(interaction, reply))
            .await
    }

    async fn followup(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        self.race(true, self.inner.followup(interaction, reply))
            .await
    }

    async fn autocomplete(
        &self,
        interaction: &InteractionRef,
        choices: &[CommandOptionChoice],
    ) -> Outcome<()> {
        self.race(true, self.inner.autocomplete(interaction, choices))
            .await
    }

    async fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> Outcome<()> {
        self.race(true, self.inner.register_guild_commands(guild, commands))
            .await
    }

    async fn list_members(
        &self,
        guild: Id<GuildMarker>,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Member>> {
        self.race(false, self.inner.list_members(guild, after, limit))
            .await
    }

    async fn channel_messages(
        &self,
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
    ) -> Outcome<Vec<Message>> {
        self.race(false, self.inner.channel_messages(channel, page, limit))
            .await
    }

    async fn guild_channels(&self, guild: Id<GuildMarker>) -> Outcome<Vec<Channel>> {
        self.race(false, self.inner.guild_channels(guild)).await
    }

    async fn current_user(&self) -> Outcome<CurrentUser> {
        self.race(false, self.inner.current_user()).await
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::super::super::budget::{SEND_FINALISE, STORE_RESERVE, TOTAL};
    use super::*;
    use crate::bot::transport::{FakeDiscord, Op};

    fn message() -> OutgoingMessage {
        OutgoingMessage {
            content: Some("synthetic".into()),
            embeds: Vec::new(),
            allowed_mentions: crate::bot::mentions::none(),
            reply_to: None,
            attachments: Vec::new(),
        }
    }

    /// A pass-through until shutdown; then a write in flight at the send cut
    /// may have landed (`Timeout`), and every later call is not sent.
    #[tokio::test]
    async fn calls_pass_through_until_the_send_cut_then_end_as_a_timeout() {
        let fake = Arc::new(FakeDiscord::new());
        let clock = ShutdownClock::default();
        let transport = StopAware::new(Arc::clone(&fake), clock.clone());
        let channel = Id::new(301);
        assert!(matches!(
            transport.create_message(channel, &message()).await,
            Outcome::Delivered(_)
        ));

        let hold = fake.hold(Op::Create);
        let held = message();
        let left = Duration::from_millis(200);
        let (outcome, ()) = tokio::join!(transport.create_message(channel, &held), async {
            hold.entered().await;
            clock.start_at(Instant::now() - (TOTAL - STORE_RESERVE - SEND_FINALISE - left));
        });
        assert!(
            matches!(outcome, Outcome::Ambiguous(AmbiguousKind::Timeout)),
            "{outcome:?}"
        );
        let started = Instant::now();
        assert!(matches!(
            transport.create_message(channel, &message()).await,
            Outcome::DefinitelyRejected(RejectionKind::NotSent)
        ));
        assert!(matches!(
            transport.guild_channels(Id::new(900)).await,
            Outcome::DefinitelyRejected(RejectionKind::NotSent)
        ));
        assert!(started.elapsed() < Duration::from_millis(100));
        // Only the first create completed: the cut one never finished and
        // none began after the cut.
        assert_eq!(fake.count(Op::Create), 1);
        hold.release();
    }
}
