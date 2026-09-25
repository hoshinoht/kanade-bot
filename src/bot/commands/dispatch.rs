//! Command registry and dispatch: gate first, then run, with every refusal
//! or failure answered ephemerally and without mentions. Autocomplete goes
//! through the same gate (a refusal lists nothing), and a redelivered
//! interaction id is answered only once.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::task::JoinHandle;

use twilight_model::application::command::{
    Command, CommandOptionChoice, CommandOptionChoiceValue,
};
use twilight_model::application::interaction::Interaction;
use twilight_model::id::{
    Id,
    marker::{GuildMarker, InteractionMarker, UserMarker},
};

use super::access::{AccessPolicy, Denial, Gate};
use super::invocation::Invocation;
use crate::bot::transport::{DiscordTransport, InteractionReply, Outcome, RejectionKind};

/// v4's reply for unexpected failures.
pub const GENERIC_FAILURE: &str = "❌ Something went wrong. Check the bot logs.";

pub type CommandFuture<'a> =
    Pin<Box<dyn Future<Output = Result<InteractionReply, CommandError>> + Send + 'a>>;

pub type ChoicesFuture<'a> = Pin<Box<dyn Future<Output = Vec<CommandOptionChoice>> + Send + 'a>>;

/// Discord's autocomplete limit.
pub const MAX_CHOICES: usize = 25;

/// Interaction ids remembered to drop redeliveries.
const SEEN_INTERACTIONS: usize = 512;

/// A string choice; the name is cut to Discord's 100 characters.
pub fn choice(name: &str, value: impl Into<String>) -> CommandOptionChoice {
    CommandOptionChoice {
        name: name.chars().take(100).collect(),
        name_localizations: None,
        value: CommandOptionChoiceValue::String(value.into()),
    }
}

/// Why a command did not produce its reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandError {
    /// Shown to the invoker as `❌ {message}`.
    User(String),
    /// Hidden behind [`GENERIC_FAILURE`]; the detail is for logs only.
    Internal(String),
}

/// One top-level command with its subcommands. The gate covers the whole
/// tree, as v4's group `interaction_check`s and per-command checks did.
pub trait SlashCommand: Send + Sync {
    /// The registration payload.
    fn definition(&self) -> Command;

    fn gate(&self) -> Gate;

    /// `Some(ephemeral)` to acknowledge with a deferred response (type 5)
    /// before running, for commands that may exceed Discord's 3 s window.
    /// Visibility is fixed at deferral. `None` answers directly.
    fn defer(&self) -> Option<bool> {
        None
    }

    /// Run an already-authorised invocation whose `path[0]` is this command.
    fn run<'a>(&'a self, invocation: &'a Invocation) -> CommandFuture<'a>;

    /// Choices for the focused option of an authorised autocomplete request.
    /// Must not fail: an error lists nothing.
    fn autocomplete<'a>(&'a self, invocation: &'a Invocation) -> ChoicesFuture<'a> {
        let _ = invocation;
        Box::pin(async { Vec::new() })
    }
}

/// What dispatch did, for the caller to log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Disposition {
    Ran,
    Denied(Denial),
    UserError,
    Failed(String),
    Unknown,
    /// The deferral was not delivered, so the command did not run.
    NotAcknowledged,
    /// Choices were offered for an autocomplete request.
    Suggested,
    /// This interaction id was already handled; nothing was sent.
    Duplicate,
}

/// What handling an interaction did and the last transport outcome; `None`
/// when it was not a guild slash command for this guild.
pub type Handled = Option<(Disposition, Outcome<()>)>;

/// A second command with the same top-level name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateCommand(pub String);

impl fmt::Display for DuplicateCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "command /{} registered twice", self.0)
    }
}

impl std::error::Error for DuplicateCommand {}

pub struct Dispatcher {
    policy: AccessPolicy,
    commands: Vec<(String, Box<dyn SlashCommand>)>,
    seen: Mutex<VecDeque<Id<InteractionMarker>>>,
}

impl fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Dispatcher")
            .field("policy", &self.policy)
            .field(
                "commands",
                &self
                    .commands
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Dispatcher {
    pub fn new(policy: AccessPolicy) -> Self {
        Self {
            policy,
            commands: Vec::new(),
            seen: Mutex::new(VecDeque::new()),
        }
    }

    pub fn policy(&self) -> &AccessPolicy {
        &self.policy
    }

    /// Record `id`; `false` when it was already seen.
    fn first_delivery(&self, id: Id<InteractionMarker>) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.contains(&id) {
            return false;
        }
        seen.push_back(id);
        while seen.len() > SEEN_INTERACTIONS {
            seen.pop_front();
        }
        true
    }

    /// # Errors
    /// [`DuplicateCommand`] when the name is taken.
    pub fn register(
        mut self,
        command: impl SlashCommand + 'static,
    ) -> Result<Self, DuplicateCommand> {
        let name = command.definition().name;
        if self.commands.iter().any(|(existing, _)| *existing == name) {
            return Err(DuplicateCommand(name));
        }
        self.commands.push((name, Box::new(command)));
        Ok(self)
    }

    /// Guild-scoped registration payloads, in registration order.
    pub fn definitions(&self) -> Vec<Command> {
        self.commands
            .iter()
            .map(|(_, command)| command.definition())
            .collect()
    }

    /// The command to run, or the refusal for an unknown or unauthorised
    /// invocation.
    fn authorise(
        &self,
        invocation: &Invocation,
        owner_id: Option<Id<UserMarker>>,
    ) -> Result<&dyn SlashCommand, (InteractionReply, Disposition)> {
        let name = invocation.path.first().map_or("", String::as_str);
        let Some((_, command)) = self.commands.iter().find(|(known, _)| known == name) else {
            return Err((
                InteractionReply::ephemeral(GENERIC_FAILURE),
                Disposition::Unknown,
            ));
        };
        match self
            .policy
            .check(command.gate(), &invocation.invoker, owner_id)
        {
            Ok(()) => Ok(command.as_ref()),
            Err(denial) => Err((
                InteractionReply::ephemeral(denial.message(name)),
                Disposition::Denied(denial),
            )),
        }
    }

    async fn execute(
        command: &dyn SlashCommand,
        invocation: &Invocation,
    ) -> (InteractionReply, Disposition) {
        match command.run(invocation).await {
            Ok(reply) => (reply, Disposition::Ran),
            Err(CommandError::User(message)) => (
                InteractionReply::ephemeral(format!("❌ {message}")),
                Disposition::UserError,
            ),
            Err(CommandError::Internal(detail)) => (
                InteractionReply::ephemeral(GENERIC_FAILURE),
                Disposition::Failed(detail),
            ),
        }
    }

    /// Authorise and run; never fails, since every outcome has a reply.
    pub async fn reply(
        &self,
        invocation: &Invocation,
        owner_id: Option<Id<UserMarker>>,
    ) -> (InteractionReply, Disposition) {
        let mut invocation = invocation.clone();
        invocation.owner_id = owner_id;
        match self.authorise(&invocation, owner_id) {
            Ok(command) => Self::execute(command, &invocation).await,
            Err(refusal) => refusal,
        }
    }

    /// Autocomplete choices after the command's gate; a refused or unknown
    /// command lists nothing.
    pub async fn suggest(
        &self,
        invocation: &Invocation,
        owner_id: Option<Id<UserMarker>>,
    ) -> Vec<CommandOptionChoice> {
        let mut invocation = invocation.clone();
        invocation.owner_id = owner_id;
        match self.authorise(&invocation, owner_id) {
            Ok(command) => {
                let mut choices = command.autocomplete(&invocation).await;
                choices.truncate(MAX_CHOICES);
                choices
            }
            Err(_) => Vec::new(),
        }
    }

    /// Answer a guild command interaction through `transport`. Returns `None`
    /// for interactions that are not guild slash commands for `guild`.
    ///
    /// Refusals answer at once. A deferring command is acknowledged first and
    /// runs only if the acknowledgement was delivered; the returned outcome
    /// is the last transport call's.
    pub async fn handle<T: DiscordTransport>(
        &self,
        transport: &T,
        guild: Id<GuildMarker>,
        interaction: &Interaction,
        owner_id: Option<Id<UserMarker>>,
    ) -> Handled {
        let mut invocation = Invocation::from_interaction(interaction)
            .filter(|invocation| invocation.guild_id == guild)?;
        invocation.owner_id = owner_id;
        if !self.first_delivery(interaction.id) {
            return Some((
                Disposition::Duplicate,
                Outcome::DefinitelyRejected(RejectionKind::NotSent),
            ));
        }
        let target = &invocation.interaction;
        if invocation.autocomplete {
            let choices = self.suggest(&invocation, owner_id).await;
            return Some((
                Disposition::Suggested,
                transport.autocomplete(target, &choices).await,
            ));
        }
        let command = match self.authorise(&invocation, owner_id) {
            Ok(command) => command,
            Err((reply, disposition)) => {
                return Some((disposition, transport.respond(target, &reply).await));
            }
        };
        let Some(ephemeral) = command.defer() else {
            let (reply, disposition) = Self::execute(command, &invocation).await;
            return Some((disposition, transport.respond(target, &reply).await));
        };
        let acknowledged = transport.defer(target, ephemeral).await;
        if !acknowledged.is_delivered() {
            return Some((Disposition::NotAcknowledged, acknowledged));
        }
        let (reply, disposition) = Self::execute(command, &invocation).await;
        Some((
            disposition,
            transport.complete_deferred(target, &reply).await,
        ))
    }
}

/// Answer an interaction on its own task so the gateway loop keeps polling.
pub fn spawn_interaction<T: DiscordTransport + 'static>(
    dispatcher: Arc<Dispatcher>,
    transport: Arc<T>,
    guild: Id<GuildMarker>,
    interaction: Box<Interaction>,
    owner_id: Option<Id<UserMarker>>,
) -> JoinHandle<Handled> {
    tokio::spawn(async move {
        dispatcher
            .handle(transport.as_ref(), guild, &interaction, owner_id)
            .await
    })
}
