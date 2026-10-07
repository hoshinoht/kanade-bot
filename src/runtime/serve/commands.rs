//! The slash commands serve registers (guild bulk overwrite on the first
//! guild availability after each ready) and dispatches: S11's retained set
//! over the same store, writer and settings the API uses. `/debug ping`
//! posts through a [`DebugDesk`] over the tick's card kit and live settings.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use crate::{
    api::state::ApiState,
    bot::{
        commands::{
            ChatAllowance, CommandContext, DebugCards, Dispatcher, DuplicateCommand, MemberRows,
            register_retained,
        },
        delivery::{AlertThrottle, DebugDesk, cards::CardKit},
        guild_cache::GuildCache,
        handler::CommandsFn,
        roster::LiveRoster,
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
    debug: Arc<dyn DebugCards>,
}

/// What `/debug ping` shares with the tick: its card kit, the live roster
/// and the live quiet-mode and post-channel settings.
pub struct DebugParts {
    pub cards: CardKit,
    pub roster: Arc<LiveRoster>,
    pub quiet: Arc<AtomicBool>,
    pub post_channel: Arc<RwLock<Option<String>>>,
    /// `[discord] test_channel`.
    pub test_channel: Option<String>,
    pub instance_id: String,
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
            config: state.config.clone(),
            rescans: state.rescans.as_ref().map(|desk| Arc::clone(&desk.runner)),
            allowance: state
                .chat
                .clone()
                .map(|chat| chat as Arc<dyn ChatAllowance>),
            debug_cards: Some(Arc::clone(&self.debug)),
            decline_retraction: state.decline_retraction.clone(),
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
    debug: DebugParts,
) -> Result<CommandsFn, Error> {
    let desk: Arc<dyn DebugCards> = Arc::new(DebugDesk {
        store: Arc::clone(&members),
        transport: Arc::clone(&transport),
        members: debug.roster,
        channels: Arc::clone(&channels) as _,
        cards: debug.cards,
        policy: state.policy.clone(),
        quiet: debug.quiet,
        post_channel: debug.post_channel,
        test_channel: debug.test_channel,
        instance_id: debug.instance_id,
        now: Arc::clone(&state.clock),
        throttle: AlertThrottle::new(),
    });
    let commands = Commands {
        state,
        members,
        channels,
        transport,
        debug: desk,
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
