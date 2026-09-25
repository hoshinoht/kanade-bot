//! A deterministic in-memory Discord for tests: records every call, applies
//! scripted outcomes per operation and assigns sequential message ids.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, MutexGuard, PoisonError};

use twilight_model::application::command::Command;
use twilight_model::id::{Id, marker::GuildMarker};

use super::{
    AmbiguousKind, ChannelId, DiscordTransport, InteractionRef, InteractionReply, MessageEdit,
    MessageId, Outcome, OutgoingMessage, Presence, RejectionKind,
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
    Register,
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
    Register {
        guild: Id<GuildMarker>,
        commands: Vec<Command>,
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
            Self::Register { .. } => Op::Register,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    next_id: u64,
    /// `None` uses [`FIRST_MESSAGE_ID`].
    first_id: Option<u64>,
    calls: Vec<Call>,
    scripts: BTreeMap<Op, VecDeque<Step>>,
    /// Applied when an operation's script is empty; `Succeed` if unset.
    defaults: BTreeMap<Op, Step>,
    /// Messages that exist remotely, by id, with their channel.
    messages: BTreeMap<MessageId, ChannelId>,
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
        let mut state = self.state();
        let outcome = match state.next_step(Op::Create) {
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
        };
        state.calls.push(Call::Create {
            channel,
            message: message.clone(),
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
        let mut state = self.state();
        let outcome = state.plain(Op::CompleteDeferred);
        state.calls.push(Call::CompleteDeferred {
            interaction: interaction.clone(),
            reply: reply.clone(),
            outcome: outcome.clone(),
        });
        outcome
    }

    async fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> Outcome<()> {
        let mut state = self.state();
        let outcome = state.plain(Op::Register);
        state.calls.push(Call::Register {
            guild,
            commands: commands.to_vec(),
            outcome: outcome.clone(),
        });
        outcome
    }
}
