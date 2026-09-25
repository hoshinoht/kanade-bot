//! The admin API over the owned store: files, settings, one shared writer,
//! sign-in and the staff gate.

use std::sync::Arc;

use twilight_model::id::Id;

use super::{health::LiveHealth, settings};
use crate::{
    api::{
        auth::{
            self, AdminAuth, Clock,
            staff::{GuildStaffGate, StoreGuildMembers},
        },
        server::LiveAdmin,
        state::{ApiState, ChannelList, GuildAccess},
        write::{ApiClock, SchedulerWriter},
    },
    bot::commands::AccessPolicy,
    chat::persona::PersonaSnapshot,
    domain::{ids::RandomIds, scheduler::SchedulerService, settings::RuntimeSettings},
    infrastructure::{
        files::{LoadError, load_catalog, load_knowledge_dir, load_personas},
        store::SqliteStore,
    },
    runtime::{
        config::{GuildSettings, ServeConfig},
        error::Error,
    },
};

/// Everything later wiring (gateway, roster, tick, chat) shares with the API.
pub struct Composition {
    pub admin: LiveAdmin,
    /// One instance for the API, the staff gate and the gateway's owner/roster hooks.
    pub access: Arc<GuildAccess>,
    pub settings: RuntimeSettings,
    pub personas: PersonaSnapshot,
}

fn file_error(error: LoadError) -> Error {
    Error::Startup(error.to_string())
}

fn access(guild: &GuildSettings) -> GuildAccess {
    // Snowflakes are validated non-zero by the config parser.
    GuildAccess::new(
        AccessPolicy {
            bossing_role_id: Id::new(guild.bossing_role_id),
            admin_role_id: guild.admin_role_id.map(Id::new),
            debug_user_ids: guild.debug_user_ids.iter().copied().map(Id::new).collect(),
        },
        guild.chat_pilot_role_id.map(|id| id.to_string()),
    )
}

pub async fn compose(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    channels: Arc<dyn ChannelList>,
) -> Result<Composition, Error> {
    let catalog = load_catalog(&config.files.catalog_file).map_err(file_error)?;
    let knowledge = config
        .files
        .knowledge_dir
        .as_deref()
        .map(load_knowledge_dir)
        .transpose()
        .map_err(file_error)?;
    let settings = settings::load(&store, &config.seeds).await?;
    let personas = load_personas(
        &config.files.persona_dir,
        settings::persona(&settings)?.as_ref(),
    )
    .map_err(file_error)?;
    let policy = settings.schedule_policy(config.runtime.timezone);

    let access = Arc::new(access(&config.guild));
    let staff = GuildStaffGate::new(
        access.policy.clone(),
        Arc::new(StoreGuildMembers::new(store.clone(), access.clone())),
    );
    let auth: AdminAuth =
        auth::from_settings(&config.runtime.admin_auth, store.clone(), Arc::new(staff))?;

    let clock: Clock = Arc::new(auth::system_now);
    let writer = SchedulerWriter::new(
        SchedulerService::new(store.clone(), RandomIds, ApiClock(clock.clone()))
            .with_attendance(policy.attendance),
    );
    let state = ApiState {
        store: store.clone(),
        writer: Arc::new(writer),
        policy,
        catalog: Arc::new(catalog),
        channels,
        personas: personas.options,
        access: access.clone(),
        knowledge_dir: knowledge.map(|dir| dir.path),
        guild_id: Some(config.guild.guild_id.to_string()),
        clock,
        rescans: None,
    };
    Ok(Composition {
        admin: LiveAdmin {
            auth: Arc::new(auth),
            state: Arc::new(state),
            health: Arc::new(LiveHealth::new(store)),
        },
        access,
        settings,
        personas: personas.snapshot,
    })
}
