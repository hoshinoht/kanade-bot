//! Slash commands: guild-scoped registration payloads, v4's access gates, a
//! dispatcher that answers refusals and failures ephemerally, and the v4
//! commands v5 retains (user decision 2026-09-25). Every write goes through
//! the shared scheduler writer; notices reach the outbox with the change.
//! Presses of the bot's Components V2 buttons are dispatched here too
//! (`components.rs`).

mod access;
mod build;
mod components;
mod context;
mod debug;
mod dispatch;
mod fixed;
mod fixed_owner;
mod invocation;
mod limits;
mod lookup;
mod members;
mod options;
mod participants;
mod rescan;
mod run_prompt;
mod runs;
mod say;
mod schedule;
mod split;
pub(crate) mod text;

use std::sync::Arc;

pub use access::{AccessPolicy, Denial, Gate, Invoker};
pub use components::{CardPresses, INACTIVE, NOT_YOURS, Press};
pub use context::{
    ChatAllowance, Clock, CommandContext, DebugCards, GuildChannels, HeaderNote, HeaderRequest,
    HeaderTrialKind, HeaderTrials, MemberRows, PingRequest, PortFuture, SampleRun, TestKind,
    TestPosted, TestReport, TestSubject,
};
pub use debug::{DebugCommand, TEST_CARDS_UNAVAILABLE, TEST_PREFIX};
pub use dispatch::{
    COMPLETION_FALLBACK, ChoicesFuture, CommandError, CommandFuture, Dispatcher, Disposition,
    DuplicateCommand, GENERIC_FAILURE, Handled, MAX_CHOICES, SlashCommand, choice,
    spawn_interaction,
};
pub use fixed::FixedCommand;
pub use invocation::Invocation;
pub use limits::{LIMITS_UNAVAILABLE, LimitsCommand, STAFF_LIMITS_REPLY, limits_text, usage_bar};
pub use members::{MemberCommand, ping_help};
pub use rescan::{RESCAN_UNAVAILABLE, RescanCommand, window_label};
pub use run_prompt::{ALREADY_ANSWERED, NOT_ON_RUN};
pub use runs::RunCommand;
pub use say::{SAY_LIMIT, SayCommand, mentioned_users};
pub use schedule::ScheduleCommand;
pub use split::{CONTENT_LIMIT, fit_embed, split_lines, split_reply};

use crate::bot::transport::DiscordTransport;

/// Every retained command, in v4's registration order (dropped and merged
/// commands are absent). `serve` registers `definitions()` for the guild only.
///
/// # Errors
/// [`DuplicateCommand`] if `dispatcher` already holds one of these names.
pub fn register_retained<T: DiscordTransport + 'static>(
    dispatcher: Dispatcher,
    ctx: &Arc<CommandContext>,
    transport: Arc<T>,
) -> Result<Dispatcher, DuplicateCommand> {
    dispatcher
        .register(FixedCommand::new(Arc::clone(ctx)))?
        .register(DebugCommand::new(Arc::clone(ctx)))?
        .register(ScheduleCommand::new(Arc::clone(ctx)))?
        .register(RunCommand::amend(Arc::clone(ctx)))?
        .register(RunCommand::swap(Arc::clone(ctx)))?
        .register(RunCommand::status(Arc::clone(ctx)))?
        .register(RunCommand::rsvp(Arc::clone(ctx)))?
        .register(MemberCommand::nick(Arc::clone(ctx)))?
        .register(MemberCommand::pings(Arc::clone(ctx)))?
        .register(MemberCommand::style(Arc::clone(ctx)))?
        .register(LimitsCommand::new(Arc::clone(ctx)))?
        .register(RescanCommand::new(Arc::clone(ctx)))?
        .register(SayCommand::new(Arc::clone(ctx), transport))
}
