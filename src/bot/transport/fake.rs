//! A deterministic in-memory Discord for tests: records every call, applies
//! scripted outcomes per operation, assigns sequential message ids and can
//! park a call mid-flight ([`FakeDiscord::hold`]).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;
use twilight_model::application::command::{Command, CommandOptionChoice};
use twilight_model::channel::message::MessageFlags;
use twilight_model::channel::{Channel, Message};
use twilight_model::guild::Member;
use twilight_model::id::{
    Id,
    marker::{GuildMarker, UserMarker},
};

use super::{
    AmbiguousKind, CREATE_FLAGS, ChannelId, DiscordTransport, HistoryPage, InteractionRef,
    InteractionReply, MAX_MEMBERS_PAGE, MAX_MESSAGES_PAGE, MessageEdit, MessageId, Outcome,
    OutgoingMessage, Presence, RejectionKind,
};

/// Operation kinds a [`Step`] can be scripted for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Op {
    Create,
    Edit,
    Delete,
    AddReaction,
    RemoveReaction,
    Presence,
    Respond,
    Defer,
    CompleteDeferred,
    Followup,
    Autocomplete,
    Register,
    ListMembers,
    ChannelMessages,
    GuildChannels,
    Typing,
}

/// A scripted result for the next call of one [`Op`]. Unscripted calls
/// succeed against the fake's own message table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Succeed,
    Reject(RejectionKind),
    /// `applied` says whether the effect really happened remotely (e.g. an
    /// ambiguous create that did post a message).
    Ambiguous {
        kind: AmbiguousKind,
        applied: bool,
    },
}

/// One recorded call with the outcome returned to the caller.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Create {
        channel: ChannelId,
        message: OutgoingMessage,
        outcome: Outcome<MessageId>,
    },
    Edit {
        channel: ChannelId,
        message: MessageId,
        edit: MessageEdit,
        outcome: Outcome<()>,
    },
    Delete {
        channel: ChannelId,
        message: MessageId,
        outcome: Outcome<()>,
    },
    AddReaction {
        channel: ChannelId,
        message: MessageId,
        emoji: String,
        outcome: Outcome<()>,
    },
    RemoveReaction {
        channel: ChannelId,
        message: MessageId,
        emoji: String,
        outcome: Outcome<()>,
    },
    Presence {
        channel: ChannelId,
        message: MessageId,
        outcome: Outcome<Presence>,
    },
    Respond {
        interaction: InteractionRef,
        reply: InteractionReply,
        outcome: Outcome<()>,
    },
    Defer {
        interaction: InteractionRef,
        ephemeral: bool,
        outcome: Outcome<()>,
    },
    CompleteDeferred {
        interaction: InteractionRef,
        reply: InteractionReply,
        outcome: Outcome<()>,
    },
    Followup {
        interaction: InteractionRef,
        reply: InteractionReply,
        outcome: Outcome<()>,
    },
    Autocomplete {
        interaction: InteractionRef,
        choices: Vec<CommandOptionChoice>,
        outcome: Outcome<()>,
    },
    Register {
        guild: Id<GuildMarker>,
        commands: Vec<Command>,
        outcome: Outcome<()>,
    },
    ListMembers {
        guild: Id<GuildMarker>,
        after: Option<Id<UserMarker>>,
        limit: u16,
        outcome: Outcome<Vec<Member>>,
    },
    ChannelMessages {
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
        outcome: Outcome<Vec<Message>>,
    },
    GuildChannels {
        guild: Id<GuildMarker>,
        outcome: Outcome<Vec<Channel>>,
    },
    Typing {
        channel: ChannelId,
        outcome: Outcome<()>,
    },
}

impl Call {
    pub fn op(&self) -> Op {
        match self {
            Self::Create { .. } => Op::Create,
            Self::Edit { .. } => Op::Edit,
            Self::Delete { .. } => Op::Delete,
            Self::AddReaction { .. } => Op::AddReaction,
            Self::RemoveReaction { .. } => Op::RemoveReaction,
            Self::Presence { .. } => Op::Presence,
            Self::Respond { .. } => Op::Respond,
            Self::Defer { .. } => Op::Defer,
            Self::CompleteDeferred { .. } => Op::CompleteDeferred,
            Self::Followup { .. } => Op::Followup,
            Self::Autocomplete { .. } => Op::Autocomplete,
            Self::Register { .. } => Op::Register,
            Self::ListMembers { .. } => Op::ListMembers,
            Self::ChannelMessages { .. } => Op::ChannelMessages,
            Self::GuildChannels { .. } => Op::GuildChannels,
            Self::Typing { .. } => Op::Typing,
        }
    }
}

/// A barrier for one call: the call parks before its step is taken or its
/// effect applied, until [`Hold::release`]. Dropping the parked call's future
/// cancels it with no effect and nothing recorded.
#[derive(Clone, Debug, Default)]
pub struct Hold {
    gate: Arc<Gate>,
}

#[derive(Debug, Default)]
struct Gate {
    entered: Notify,
    released: Notify,
}

impl Hold {
    /// Wait until the held call has arrived and parked.
    pub async fn entered(&self) {
        self.gate.entered.notified().await;
    }

    /// Let the held call proceed (also before it arrives).
    pub fn release(&self) {
        self.gate.released.notify_one();
    }

    async fn park(&self) {
        self.gate.entered.notify_one();
        self.gate.released.notified().await;
    }
}

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    /// `None` uses [`FIRST_MESSAGE_ID`].
    first_id: Option<u64>,
    calls: Vec<Call>,
    /// The flags of each recorded create, in call order (empty when plain).
    create_flags: Vec<MessageFlags>,
    /// Calls to park, per operation, in arrival order.
    holds: BTreeMap<Op, VecDeque<Hold>>,
    scripts: BTreeMap<Op, VecDeque<Step>>,
    /// Applied when an operation's script is empty; `Succeed` if unset.
    defaults: BTreeMap<Op, Step>,
    /// Messages that exist remotely, by id, with their channel.
    messages: BTreeMap<MessageId, ChannelId>,
    /// What the read operations serve, per guild or channel.
    members: BTreeMap<Id<GuildMarker>, Vec<Member>>,
    history: BTreeMap<ChannelId, Vec<Message>>,
    channels: BTreeMap<Id<GuildMarker>, Vec<Channel>>,
}

/// First id handed out; large enough to look like a real snowflake.
const FIRST_MESSAGE_ID: u64 = 1_000_000_000_000_000_001;

#[derive(Debug, Default)]
pub struct FakeDiscord {
    state: Mutex<State>,
}

impl FakeDiscord {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint message ids from `first` upwards.
    pub fn with_first_message_id(first: u64) -> Self {
        let fake = Self::default();
        fake.state().first_id = Some(first);
        fake
    }

    /// Use `step` for every unscripted call of `op` until changed; `None`
    /// restores success.
    pub fn set_default(&self, op: Op, step: Option<Step>) {
        let mut state = self.state();
        match step {
            Some(step) => state.defaults.insert(op, step),
            None => state.defaults.remove(&op),
        };
    }

    /// How many calls of `op` were made.
    pub fn count(&self, op: Op) -> usize {
        self.state()
            .calls
            .iter()
            .filter(|call| call.op() == op)
            .count()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue `step` for the next unscripted call of `op`.
    pub fn script(&self, op: Op, step: Step) {
        self.state().scripts.entry(op).or_default().push_back(step);
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state().calls.clone()
    }

    /// The flags each [`Call::Create`] was sent with, in call order (empty
    /// for [`DiscordTransport::create_message`]).
    pub fn create_flags(&self) -> Vec<MessageFlags> {
        self.state().create_flags.clone()
    }

    /// Park the next call of `op` (after any earlier holds of `op` are
    /// consumed) until the returned handle is released.
    pub fn hold(&self, op: Op) -> Hold {
        let hold = Hold::default();
        self.state()
            .holds
            .entry(op)
            .or_default()
            .push_back(hold.clone());
        hold
    }

    async fn gate(&self, op: Op) {
        let hold = self
            .state()
            .holds
            .get_mut(&op)
            .and_then(VecDeque::pop_front);
        if let Some(hold) = hold {
            hold.park().await;
        }
    }

    async fn create(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> Outcome<MessageId> {
        self.gate(Op::Create).await;
        let mut state = self.state();
        let outcome = if CREATE_FLAGS.contains(flags) {
            match state.next_step(Op::Create) {
                Step::Succeed => {
                    let id = state.mint();
                    state.messages.insert(id, channel);
                    Outcome::Delivered(id)
                }
                Step::Reject(kind) => Outcome::DefinitelyRejected(kind),
                Step::Ambiguous { kind, applied } => {
                    if applied {
                        let id = state.mint();
                        state.messages.insert(id, channel);
                    }
                    Outcome::Ambiguous(kind)
                }
            }
        } else {
            // Refused unsent like the real transport; no step is consumed.
            Outcome::DefinitelyRejected(RejectionKind::Invalid)
        };
        state.calls.push(Call::Create {
            channel,
            message: message.clone(),
            outcome: outcome.clone(),
        });
        state.create_flags.push(flags);
        outcome
    }

    /// Messages that exist remotely, including ambiguous posts that landed.
    pub fn messages(&self) -> Vec<(ChannelId, MessageId)> {
        self.state()
            .messages
            .iter()
            .map(|(message, channel)| (*channel, *message))
            .collect()
    }

    /// Pretend a message already exists (e.g. posted before a restart).
    pub fn seed_message(&self, channel: ChannelId, message: MessageId) {
        self.state().messages.insert(message, channel);
    }

    /// Members [`DiscordTransport::list_members`] pages through.
    pub fn seed_members(&self, guild: Id<GuildMarker>, members: Vec<Member>) {
        self.state().members.insert(guild, members);
    }

    /// History [`DiscordTransport::channel_messages`] pages through, keyed
    /// by each message's `channel_id`.
    pub fn seed_history(&self, messages: Vec<Message>) {
        let mut state = self.state();
        for message in messages {
            state
                .history
                .entry(message.channel_id)
                .or_default()
                .push(message);
        }
    }

    /// Channels [`DiscordTransport::guild_channels`] returns.
    pub fn seed_channels(&self, guild: Id<GuildMarker>, channels: Vec<Channel>) {
        self.state().channels.insert(guild, channels);
    }
}

impl State {
    fn next_step(&mut self, op: Op) -> Step {
        self.scripts
            .get_mut(&op)
            .and_then(VecDeque::pop_front)
            .or_else(|| self.defaults.get(&op).cloned())
            .unwrap_or(Step::Succeed)
    }

    fn mint(&mut self) -> MessageId {
        let id = Id::new(self.first_id.unwrap_or(FIRST_MESSAGE_ID) + self.next_id);
        self.next_id += 1;
        id
    }

    /// Resolve a call on an existing message: missing messages are Unknown
    /// Message, and `apply` runs when the effect happens.
    fn on_message(
        &mut self,
        op: Op,
        channel: ChannelId,
        message: MessageId,
        apply: impl FnOnce(&mut Self),
    ) -> Outcome<()> {
        let step = self.next_step(op);
        let exists = self.messages.get(&message) == Some(&channel);
        match step {
            Step::Reject(kind) => Outcome::DefinitelyRejected(kind),
            _ if !exists => Outcome::DefinitelyRejected(RejectionKind::UnknownMessage),
            Step::Succeed => {
                apply(self);
                Outcome::Delivered(())
            }
            Step::Ambiguous { kind, applied } => {
                if applied {
                    apply(self);
                }
                Outcome::Ambiguous(kind)
            }
        }
    }

    /// A read: an invalid limit is refused before any step is consumed (as
    /// Twilight validates before sending); a success serves `data`.
    fn read<T>(&mut self, op: Op, valid: bool, data: impl FnOnce(&Self) -> T) -> Outcome<T> {
        if !valid {
            return Outcome::DefinitelyRejected(RejectionKind::Invalid);
        }
        match self.next_step(op) {
            Step::Succeed => Outcome::Delivered(data(self)),
            Step::Reject(kind) => Outcome::DefinitelyRejected(kind),
            Step::Ambiguous { kind, .. } => Outcome::Ambiguous(kind),
        }
    }

    fn plain(&mut self, op: Op) -> Outcome<()> {
        match self.next_step(op) {
            Step::Succeed => Outcome::Delivered(()),
            Step::Reject(kind) => Outcome::DefinitelyRejected(kind),
            Step::Ambiguous { kind, .. } => Outcome::Ambiguous(kind),
        }
    }
}

impl DiscordTransport for FakeDiscord {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        self.create(channel, message, MessageFlags::empty()).await
    }

    async fn create_flagged_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
        flags: MessageFlags,
    ) -> Outcome<MessageId> {
        self.create(channel, message, flags).await
    }

    async fn trigger_typing(&self, channel: ChannelId) -> Outcome<()> {
        self.gate(Op::Typing).await;
        let mut state = self.state();
        let outcome = state.plain(Op::Typing);
        state.calls.push(Call::Typing {
            channel,
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> Outcome<()> {
        self.gate(Op::Edit).await;
        let mut state = self.state();
        let outcome = state.on_message(Op::Edit, channel, message, |_| {});
        state.calls.push(Call::Edit {
            channel,
            message,
            edit: edit.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        self.gate(Op::Delete).await;
        let mut state = self.state();
        let outcome = state.on_message(Op::Delete, channel, message, |state| {
            state.messages.remove(&message);
        });
        state.calls.push(Call::Delete {
            channel,
            message,
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        self.gate(Op::AddReaction).await;
        let mut state = self.state();
        let outcome = state.on_message(Op::AddReaction, channel, message, |_| {});
        state.calls.push(Call::AddReaction {
            channel,
            message,
            emoji: emoji.to_owned(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> Outcome<()> {
        self.gate(Op::RemoveReaction).await;
        let mut state = self.state();
        let outcome = state.on_message(Op::RemoveReaction, channel, message, |_| {});
        state.calls.push(Call::RemoveReaction {
            channel,
            message,
            emoji: emoji.to_owned(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn message_presence(&self, channel: ChannelId, message: MessageId) -> Outcome<Presence> {
        self.gate(Op::Presence).await;
        let mut state = self.state();
        let outcome = match state.next_step(Op::Presence) {
            Step::Reject(kind) => Outcome::DefinitelyRejected(kind),
            Step::Ambiguous { kind, .. } => Outcome::Ambiguous(kind),
            Step::Succeed if state.messages.get(&message) == Some(&channel) => {
                Outcome::Delivered(Presence::Present)
            }
            Step::Succeed => Outcome::Delivered(Presence::Absent),
        };
        state.calls.push(Call::Presence {
            channel,
            message,
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn respond(&self, interaction: &InteractionRef, reply: &InteractionReply) -> Outcome<()> {
        self.gate(Op::Respond).await;
        let mut state = self.state();
        let outcome = state.plain(Op::Respond);
        state.calls.push(Call::Respond {
            interaction: interaction.clone(),
            reply: reply.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn defer(&self, interaction: &InteractionRef, ephemeral: bool) -> Outcome<()> {
        self.gate(Op::Defer).await;
        let mut state = self.state();
        let outcome = state.plain(Op::Defer);
        state.calls.push(Call::Defer {
            interaction: interaction.clone(),
            ephemeral,
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        self.gate(Op::CompleteDeferred).await;
        let mut state = self.state();
        let outcome = state.plain(Op::CompleteDeferred);
        state.calls.push(Call::CompleteDeferred {
            interaction: interaction.clone(),
            reply: reply.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn followup(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> Outcome<()> {
        self.gate(Op::Followup).await;
        let mut state = self.state();
        let outcome = state.plain(Op::Followup);
        state.calls.push(Call::Followup {
            interaction: interaction.clone(),
            reply: reply.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn autocomplete(
        &self,
        interaction: &InteractionRef,
        choices: &[CommandOptionChoice],
    ) -> Outcome<()> {
        self.gate(Op::Autocomplete).await;
        let mut state = self.state();
        let outcome = state.plain(Op::Autocomplete);
        state.calls.push(Call::Autocomplete {
            interaction: interaction.clone(),
            choices: choices.to_vec(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> Outcome<()> {
        self.gate(Op::Register).await;
        let mut state = self.state();
        let outcome = state.plain(Op::Register);
        state.calls.push(Call::Register {
            guild,
            commands: commands.to_vec(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn list_members(
        &self,
        guild: Id<GuildMarker>,
        after: Option<Id<UserMarker>>,
        limit: u16,
    ) -> Outcome<Vec<Member>> {
        self.gate(Op::ListMembers).await;
        let mut state = self.state();
        let valid = (1..=MAX_MEMBERS_PAGE).contains(&limit);
        let outcome = state.read(Op::ListMembers, valid, |state| {
            let mut members: Vec<Member> = state
                .members
                .get(&guild)
                .into_iter()
                .flatten()
                .filter(|member| after.is_none_or(|after| member.user.id > after))
                .cloned()
                .collect();
            members.sort_by_key(|member| member.user.id);
            members.truncate(usize::from(limit));
            members
        });
        state.calls.push(Call::ListMembers {
            guild,
            after,
            limit,
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn channel_messages(
        &self,
        channel: ChannelId,
        page: HistoryPage,
        limit: u16,
    ) -> Outcome<Vec<Message>> {
        self.gate(Op::ChannelMessages).await;
        let mut state = self.state();
        let valid = (1..=MAX_MESSAGES_PAGE).contains(&limit);
        let outcome = state.read(Op::ChannelMessages, valid, |state| {
            let mut history: Vec<&Message> =
                state.history.get(&channel).into_iter().flatten().collect();
            history.sort_by_key(|message| message.id);
            let limit = usize::from(limit);
            match page {
                HistoryPage::Latest => history.iter().rev().take(limit).copied().cloned().collect(),
                HistoryPage::Before(id) => history
                    .iter()
                    .rev()
                    .filter(|message| message.id < id)
                    .take(limit)
                    .copied()
                    .cloned()
                    .collect(),
                HistoryPage::After(id) => {
                    let mut oldest: Vec<Message> = history
                        .iter()
                        .filter(|message| message.id > id)
                        .take(limit)
                        .copied()
                        .cloned()
                        .collect();
                    oldest.reverse();
                    oldest
                }
            }
        });
        state.calls.push(Call::ChannelMessages {
            channel,
            page,
            limit,
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn guild_channels(&self, guild: Id<GuildMarker>) -> Outcome<Vec<Channel>> {
        self.gate(Op::GuildChannels).await;
        let mut state = self.state();
        let outcome = state.read(Op::GuildChannels, true, |state| {
            state.channels.get(&guild).cloned().unwrap_or_default()
        });
        state.calls.push(Call::GuildChannels {
            guild,
            outcome: outcome.clone(),
        });
        outcome
    }
}
