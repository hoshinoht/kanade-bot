//! The slash commands serve registers (guild bulk overwrite on the first
//! guild availability after each ready) and dispatches: S11's retained set
//! over the same store, writer and settings the API uses, then
//! `/character` while module `character_data` is on (off, the overwrite
//! leaves it out). `/debug ping` posts through a [`DebugDesk`] over the
//! tick's card kit and live settings.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

use crate::{
    app::{
        clock::Clock,
        config::ConfigDesk,
        guild::GuildAccess,
        ports::{DeclineRetraction, HeaderRewritePort},
        rescan::RescanRunner,
        write::Writer,
    },
    bot::{
        commands::{
            CardPresses, CharacterParts, ChatAllowance, CommandContext, DebugCards, Dispatcher,
            DuplicateCommand, MemberRows, register_character, register_retained,
        },
        delivery::{AlertThrottle, DebugDesk},
        handler::CommandsFn,
    },
    cards::CardKit,
    chat::driver::ChatHandle,
    discord::{guild_cache::GuildCache, roster::LiveRoster},
    infrastructure::store::SqliteStore,
    runtime::error::Error,
};
use kanade_domain::{catalog::BossTable, schedule::SchedulePolicy};

use crate::runtime::serve::late::GatewayTransport;

/// What the commands share with the rest of serve (and the API).
pub struct Shared {
    pub store: Arc<SqliteStore>,
    /// The one scheduler writer.
    pub writer: Arc<dyn Writer>,
    pub policy: SchedulePolicy,
    pub catalog: Arc<BossTable>,
    pub access: Arc<GuildAccess>,
    pub config: Arc<ConfigDesk>,
    /// Rescan jobs; `None` while extraction has no runner.
    pub rescans: Option<Arc<dyn RescanRunner>>,
    /// The chat pilot's allowance.
    pub chat: Arc<ChatHandle>,
    pub decline_retraction: DeclineRetraction,
    pub header_rewrite: HeaderRewritePort,
    pub clock: Clock,
}

/// Everything a dispatcher is built from.
struct Commands<T> {
    shared: Shared,
    channels: Arc<GuildCache>,
    transport: Arc<T>,
    debug: Arc<dyn DebugCards>,
    /// V2 proposal-card presses, answered by the reaction worker.
    presses: CardPresses,
    /// `/character`'s parts while module `character_data` is on.
    character: Option<CharacterParts<T>>,
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
        let shared = &self.shared;
        let members: Arc<dyn MemberRows> = shared.store.clone();
        let ctx = Arc::new(CommandContext {
            store: shared.store.clone(),
            writer: Arc::clone(&shared.writer),
            members,
            run_prompts: shared.store.clone(),
            policy: shared.policy.clone(),
            catalog: Arc::clone(&shared.catalog),
            channels: self.channels.clone(),
            access: Arc::clone(&shared.access),
            config: Some(Arc::clone(&shared.config)),
            rescans: shared.rescans.clone(),
            allowance: Some(Arc::clone(&shared.chat) as Arc<dyn ChatAllowance>),
            debug_cards: Some(Arc::clone(&self.debug)),
            decline_retraction: Some(Arc::clone(&shared.decline_retraction)),
            header_rewrite: Some(Arc::clone(&shared.header_rewrite)),
            bot_name,
            clock: Arc::clone(&shared.clock),
        });
        let retained = register_retained(
            Dispatcher::new(shared.access.policy.clone())
                .with_card_presses(Arc::clone(&self.presses)),
            &ctx,
            Arc::clone(&self.transport),
        )?;
        match &self.character {
            Some(parts) => register_character(retained, &ctx, parts.clone()),
            None => Ok(retained),
        }
    }
}

/// A dispatcher factory for the gateway handler, checked once now so a
/// registry error fails startup rather than the first `READY`.
pub fn factory<T: GatewayTransport>(
    shared: Shared,
    channels: Arc<GuildCache>,
    transport: Arc<T>,
    debug: DebugParts,
    presses: CardPresses,
    character: Option<CharacterParts<T>>,
) -> Result<CommandsFn, Error> {
    let desk: Arc<dyn DebugCards> = Arc::new(DebugDesk {
        store: Arc::clone(&shared.store),
        transport: Arc::clone(&transport),
        members: debug.roster,
        channels: Arc::clone(&channels) as _,
        cards: debug.cards,
        policy: shared.policy.clone(),
        quiet: debug.quiet,
        post_channel: debug.post_channel,
        test_channel: debug.test_channel,
        instance_id: debug.instance_id,
        now: Arc::clone(&shared.clock),
        throttle: AlertThrottle::new(),
    });
    let commands = Commands {
        shared,
        channels,
        transport,
        debug: desk,
        presses,
        character,
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
