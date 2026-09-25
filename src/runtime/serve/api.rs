//! The admin API over the owned store: files, settings, one shared writer,
//! sign-in and the staff gate.

use std::sync::Arc;

use twilight_model::id::Id;

use super::{health::LiveHealth, settings};
use crate::{
    api::{
        admin::config::{ConfigDesk, ConfigFacts, ConfigInputs, ModelCatalog, PersonaFiles},
        auth::{
            self, AdminAuth, Clock,
            staff::{GuildStaffGate, StoreGuildMembers},
        },
        server::LiveAdmin,
        state::{ApiState, ChannelList, GuildAccess},
        write::{ApiClock, SchedulerWriter},
    },
    bot::commands::AccessPolicy,
    chat::persona::PersonaStore,
    domain::{ids::RandomIds, scheduler::SchedulerService, settings::RuntimeSettings},
    infrastructure::{
        files::{LoadError, load_catalog, load_knowledge_dir, load_personas},
        llm::{
            governor::XorShift,
            setup::{ModelRoles, ModelSetup, ModelStack, Models, build},
        },
        store::SqliteStore,
    },
    runtime::{
        config::{GuildSettings, ModelSettings, ServeConfig},
        error::Error,
    },
};

/// Everything later wiring (gateway, roster, tick, chat) shares with the API.
pub struct Composition {
    pub admin: LiveAdmin,
    /// One instance for the API, the staff gate and the gateway's owner/roster hooks.
    pub access: Arc<GuildAccess>,
    pub settings: RuntimeSettings,
    /// The live persona snapshot; the config API swaps it on a switch or reload.
    pub personas: Arc<PersonaStore>,
    /// `None` without `KANADE_MODEL_BASE_URL`. Role routes are fixed at build,
    /// so role changes saved in the config API apply at restart.
    pub models: Option<Arc<ModelStack>>,
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

/// A role with no stored alias runs on its `KANADE_*_MODEL` (a seed).
fn seed_aliases(settings: &mut RuntimeSettings, models: &ModelSettings) {
    let roles = &mut settings.models;
    for (role, alias) in [
        (&mut roles.extraction, &models.extract_model),
        (&mut roles.chat, &models.chat_model),
        (&mut roles.rewrite, &models.rewrite_model),
    ] {
        if role.alias.is_none() {
            role.alias.clone_from(alias);
        }
    }
}

fn model_stack(
    models: &ModelSettings,
    settings: &RuntimeSettings,
) -> Result<Option<Arc<ModelStack>>, Error> {
    if models.base_url.is_none() {
        return Ok(None);
    }
    let setup = ModelSetup {
        base_url: models.base_url.clone(),
        key: models
            .read_key()?
            .map(|key| key.expose().as_bytes().to_vec()),
        ca_file: models.ca_file.clone(),
        roles: ModelRoles::from(&settings.models),
        permits: u32::from(models.permits),
        allow_external_unmasked: models.allow_external_unmasked,
    };
    let random = Arc::new(XorShift::new(uuid::Uuid::new_v4().as_u64_pair().0));
    match build(setup, random) {
        Ok(Models::Ready(stack)) => Ok(Some(Arc::from(stack))),
        Ok(Models::Unavailable) => Ok(None),
        Err(error) => Err(Error::Startup(format!("model setup: {error}"))),
    }
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
    let mut settings = settings::load(&store, &config.seeds).await?;
    seed_aliases(&mut settings, &config.models);
    let models = model_stack(&config.models, &settings)?;
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

    let persona_store = Arc::new(PersonaStore::new(personas.snapshot));
    let desk = ConfigDesk::new(ConfigInputs {
        settings: settings.clone(),
        store: store.clone(),
        models: models.clone().map(|stack| stack as Arc<dyn ModelCatalog>),
        facts: ConfigFacts {
            timezone: config.runtime.timezone.name().to_owned(),
            model_gateway: config.models.base_url.clone(),
            model_permits: u32::from(config.models.permits),
            allow_external_unmasked: config.models.allow_external_unmasked,
            chat_pilot_role_id: config.guild.chat_pilot_role_id.map(|id| id.to_string()),
        },
        personas: Some(PersonaFiles {
            dir: config.files.persona_dir.clone(),
            store: persona_store.clone(),
        }),
    });

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
        config: Some(Arc::new(desk)),
    };
    Ok(Composition {
        admin: LiveAdmin {
            auth: Arc::new(auth),
            state: Arc::new(state),
            health: Arc::new(LiveHealth::new(store)),
        },
        access,
        settings,
        personas: persona_store,
        models,
    })
}
