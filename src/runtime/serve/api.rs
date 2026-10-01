//! The admin API over the owned store: files, settings, one shared writer,
//! sign-in and the staff gate.

use std::sync::Arc;

use twilight_model::id::Id;

use super::{
    health::LiveHealth,
    models::{self as model_report, ModelTasks},
    settings,
};
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
    domain::{
        ids::RandomIds,
        scheduler::SchedulerService,
        settings::{RuntimeSettings, SettingsStore},
    },
    infrastructure::{
        files::{LoadError, load_catalog, load_knowledge_dir, load_personas},
        llm::{
            governor::XorShift,
            setup::{ModelRoles, ModelSetup, ModelStack, Models, build_with_groups},
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
    /// `None` without `KANADE_MODEL_BASE_URL`. Role aliases and reasoning
    /// saved in the config API switch it live (next session per role).
    pub models: Option<Arc<ModelStack>>,
    /// Catalog refresh and the startup report; aborted when dropped.
    pub model_tasks: ModelTasks,
}

fn file_error(error: LoadError) -> Error {
    Error::Startup(error.to_string())
}

/// Bounds and duplicate pairs are rejected while parsing the seed. Exact boss
/// keys and difficulties need the catalog, so reject them after it has loaded
/// and only when no saved row takes precedence.
fn validate_run_lengths_seed(
    seed: Option<&crate::domain::settings::RunLengths>,
    catalog: &crate::domain::catalog::BossTable,
) -> Result<(), Error> {
    let Some(seed) = seed else {
        return Ok(());
    };
    for (index, override_) in seed.overrides.iter().enumerate() {
        let field = format!("KANADE_RUN_LENGTHS.overrides[{index}]");
        let boss = catalog.boss(&override_.boss).ok_or_else(|| {
            Error::Configuration(format!("{field}.boss is not a catalog boss key"))
        })?;
        if !boss
            .difficulties()
            .iter()
            .any(|difficulty| difficulty == &override_.difficulty)
        {
            return Err(Error::Configuration(format!(
                "{field}.difficulty is not valid for {}",
                override_.boss
            )));
        }
    }
    Ok(())
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
    };
    let random = Arc::new(XorShift::new(uuid::Uuid::new_v4().as_u64_pair().0));
    match build_with_groups(setup, &models.groups, random) {
        Ok(Models::Ready(stack)) => Ok(Some(Arc::from(stack))),
        Ok(Models::Unavailable) => Ok(None),
        Err(error) => Err(Error::Startup(format!("model setup: {error}"))),
    }
}

pub async fn compose(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    channels: Arc<dyn ChannelList>,
    health: LiveHealth,
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
    // Loaded above, so a read failure here is a transient store error.
    let stored = store
        .settings_rows()
        .await
        .map_err(|_| Error::Startup("runtime settings could not be read".into()))?;
    // An unsaved seed was applied by `load`; refuse it before anything uses it.
    if !stored.contains_key(crate::domain::settings::keys::RUN_LENGTHS) {
        validate_run_lengths_seed(config.seeds.run_lengths.as_ref(), &catalog)?;
    }
    let sources = model_report::seed_roles(&mut settings, &config.models, &stored);
    let models = model_stack(&config.models, &settings)?;
    let model_tasks =
        model_report::start(models.as_ref(), sources, settings.models.context.clone());
    let personas = load_personas(
        &config.files.persona_dir,
        settings::persona(&settings)?.as_ref(),
    )
    .map_err(file_error)?;
    super::persona_log::persona_selected(&personas.snapshot);
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
            model_groups: config.models.groups.clone(),
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
        store,
        writer: Arc::new(writer),
        policy,
        catalog: Arc::new(catalog),
        channels,
        access: access.clone(),
        knowledge_dir: knowledge.map(|dir| dir.path),
        guild_id: Some(config.guild.guild_id.to_string()),
        clock,
        rescans: None,
        config: Some(Arc::new(desk)),
        chat: Some(health.chat()),
        proposal_refresh: None,
    };
    Ok(Composition {
        admin: LiveAdmin {
            auth: Arc::new(auth),
            state: Arc::new(state),
            health: Arc::new(health),
        },
        access,
        settings,
        personas: persona_store,
        models,
        model_tasks,
    })
}
