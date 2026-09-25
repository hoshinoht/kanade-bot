//! The slash commands serve registers (guild bulk overwrite on the first
//! guild availability after each ready) and dispatches: S11's retained set
//! over the same store, writer and settings the API uses.

use std::sync::Arc;

use crate::{
    api::state::ApiState,
    bot::{
        commands::{
            ChatAllowance, CommandContext, Dispatcher, DuplicateCommand, MemberRows,
            register_retained,
        },
        guild_cache::GuildCache,
        handler::CommandsFn,
    },
    infrastructure::store::SqliteStore,
    runtime::error::Error,
};

use super::discord::GatewayTransport;

/// Everything a dispatcher is built from, shared with `ApiState`.
struct Commands<T> {
    state: Arc<ApiState>,
    members: Arc<SqliteStore>,
    channels: Arc<GuildCache>,
    transport: Arc<T>,
}

impl<T: GatewayTransport> Commands<T> {
    fn build(&self, bot_name: Option<String>) -> Result<Dispatcher, DuplicateCommand> {
        let state = &self.state;
        let members: Arc<dyn MemberRows> = self.members.clone();
        let ctx = Arc::new(CommandContext {
            store: Arc::clone(&state.store),
            writer: Arc::clone(&state.writer),
            members,
            policy: state.policy.clone(),
            catalog: Arc::clone(&state.catalog),
            channels: self.channels.clone(),
            access: Arc::clone(&state.access),
            personas: state.personas.clone(),
            rescans: None,
            allowance: state
                .chat
                .clone()
                .map(|chat| chat as Arc<dyn ChatAllowance>),
            debug_cards: None,
            bot_name,
            clock: Arc::clone(&state.clock),
        });
        register_retained(
            Dispatcher::new(state.access.policy.clone()),
            &ctx,
            Arc::clone(&self.transport),
        )
    }
}

/// A dispatcher factory for the gateway handler, checked once now so a
/// registry error fails startup rather than the first `READY`.
pub fn factory<T: GatewayTransport>(
    state: Arc<ApiState>,
    members: Arc<SqliteStore>,
    channels: Arc<GuildCache>,
    transport: Arc<T>,
) -> Result<CommandsFn, Error> {
    let commands = Commands {
        state,
        members,
        channels,
        transport,
    };
    let checked = Arc::new(
        commands
            .build(None)
            .map_err(|error| Error::Startup(error.to_string()))?,
    );
    Ok(Box::new(move |bot_name| {
        commands
            .build(bot_name)
            .map_or_else(|_| Arc::clone(&checked), Arc::new)
    }))
}
