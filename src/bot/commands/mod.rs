//! Slash-command framework: guild-scoped registration payloads, v4's access
//! gates, and a dispatcher that answers refusals and failures ephemerally.

mod access;
mod debug_status;
mod dispatch;
mod invocation;

pub use access::{AccessPolicy, Denial, Gate, Invoker};
pub use debug_status::{DebugCommand, format_uptime};
pub use dispatch::{
    CommandError, CommandFuture, Dispatcher, Disposition, DuplicateCommand, GENERIC_FAILURE,
    Handled, SlashCommand, spawn_interaction,
};
pub use invocation::Invocation;
