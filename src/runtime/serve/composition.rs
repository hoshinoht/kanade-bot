//! What both halves of serve share, composed once over the owned store:
//! files, settings, personas, the model stack, the access policy, the
//! config desk and the one scheduler writer; with the `web` feature, the
//! admin API's state and sign-in realms over them (`web::api`).

use std::sync::Arc;

use twilight_model::id::Id;

#[cfg(feature = "web")]
use crate::api::server::LiveAdmin;
#[cfg(not(feature = "web"))]
use crate::app::roster::HookFuture;
use crate::runtime::serve::{
    health::LiveHealth,
    models::{self as model_report, ModelTasks},
    settings,
};
use crate::{
    app::{
        access::AccessPolicy,
        clock::{self, Clock},
        config::{ConfigDesk, ConfigFacts, ConfigInputs, ModelCatalog, PersonaFiles},
        guild::{ChannelList, GuildAccess},
        read::CharacterRoster,
        roster::RosterHooks,
        write::{ApiClock, SchedulerWriter, Writer},
    },
    chat::persona::{PersonaStore, load_personas},
    runtime::{
        config::{GuildSettings, ModelSettings, ServeConfig},
        error::Error,
    },
};
use kanade_chardata::character_data::CharacterData;
use kanade_domain::{
    catalog::BossTable,
    ids::RandomIds,
    schedule::SchedulePolicy,
    scheduler::SchedulerService,
    settings::{RuntimeSettings, SettingsStore},
};
use kanade_files::{KnowledgeDir, LoadError, load_catalog, load_knowledge_dir};
use kanade_llm::{
    governor::XorShift,
    setup::{ModelRoles, ModelSetup, ModelStack, Models, build_with_groups},
};
use kanade_store::SqliteStore;

/// Everything later wiring (gateway, roster, tick, chat, the API) shares.
pub struct Composition {
    /// The admin and public API's state and sign-in realms.
    #[cfg(feature = "web")]
    pub admin: LiveAdmin,
    /// One instance for the API, the staff gate and the gateway's owner/roster hooks.
    pub access: Arc<GuildAccess>,
    pub settings: RuntimeSettings,
    /// The live persona snapshot; the config API swaps it on a switch or reload.
    pub personas: Arc<PersonaStore>,
    /// `None` without `KANADE_MODEL_BASE_URL`. Role aliases and reasoning
    /// saved in the config API switch it live (next session per role).
    pub models: Option<Arc<ModelStack>>,
    /// Validated once at compose time, then shared by the admin API and chat.
    pub knowledge: Option<Arc<KnowledgeDir>>,
    /// Catalog refresh and the startup report; aborted when dropped.
    pub model_tasks: ModelTasks,
    pub catalog: Arc<BossTable>,
    /// Fixed at startup, as the API's.
    pub policy: SchedulePolicy,
    /// Runtime settings (A9): every Config save and its change stream.
    pub config: Arc<ConfigDesk>,
    /// The one scheduler writer (a mutex inside), shared with the API.
    pub writer: Arc<dyn Writer>,
    /// The writer's, the API's and both sign-in realms' clock.
    pub clock: Clock,
    /// The manual character roster (`modules.roster`, read at startup).
    pub characters: Option<Arc<dyn CharacterRoster>>,
    pub health: Arc<LiveHealth>,
    /// Module `character_data`'s service for the Discord side, which takes
    /// it (set only with the `bot` component, so nothing else keeps its
    /// file open past shutdown).
    pub character_data: Option<Arc<CharacterData>>,
}

impl Composition {
    /// When runs end, live from the config desk.
    pub fn run_ends_source(&self) -> kanade_domain::completion::RunEndsSource {
        self.config
            .run_ends(Arc::clone(&self.catalog), self.policy.clone())
    }

    /// What the roster does to sign-in sessions when a member changes: both
    /// realms' re-checks and the portrait purge.
    #[cfg(feature = "web")]
    pub fn roster_hooks(&self) -> Arc<dyn RosterHooks> {
        self.admin.roster_hooks()
    }

    /// A build without the web side has no realm or portrait cache here;
    /// the web server's own staff and eligibility re-checks (every five
    /// minutes of use, against the roster this process stores) end the
    /// sessions instead.
    #[cfg(not(feature = "web"))]
    pub fn roster_hooks(&self) -> Arc<dyn RosterHooks> {
        Arc::new(NoRealms)
    }
}

#[cfg(not(feature = "web"))]
struct NoRealms;

#[cfg(not(feature = "web"))]
impl RosterHooks for NoRealms {
    fn changed<'a>(&'a self, _: &'a str, _: bool) -> HookFuture<'a> {
        Box::pin(async { 0 })
    }

    fn left<'a>(&'a self, _: &'a str) -> HookFuture<'a> {
        Box::pin(async { 0 })
    }

    fn staff_changed<'a>(&'a self, _: &'a str) -> HookFuture<'a> {
        Box::pin(async { 0 })
    }
}

fn file_error(error: LoadError) -> Error {
    Error::Startup(error.to_string())
}

/// Bounds and duplicate pairs are rejected while parsing the seed. Exact boss
/// keys and difficulties need the catalog, so reject them after it has loaded
/// and only when no saved row takes precedence.
fn validate_run_lengths_seed(
    seed: Option<&kanade_domain::settings::RunLengths>,
    catalog: &BossTable,
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

/// The model stack of `models`, its `prompt_cache_key`s scoped to `guild_id`.
pub(in crate::runtime::serve) fn model_stack(
    models: &ModelSettings,
    settings: &RuntimeSettings,
    guild_id: u64,
) -> Result<Option<Arc<ModelStack>>, Error> {
    if models.base_url.is_none() {
        return Ok(None);
    }
    let setup = ModelSetup {
        base_url: models.base_url.clone(),
        key: models
            .read_key()?
            .map(|key| key.expose().as_bytes().to_vec()),
        ca_file: models.ca_file()?,
        roles: ModelRoles::from(&settings.models),
        permits: u32::from(models.permits),
        capabilities: models.capabilities.clone(),
        cache_scope: Some(guild_id),
    };
    let random = Arc::new(XorShift::new(uuid::Uuid::new_v4().as_u64_pair().0));
    match build_with_groups(setup, &models.groups, random) {
        Ok(Models::Ready(stack)) => Ok(Some(Arc::from(stack))),
        Ok(Models::Unavailable) => Ok(None),
        Err(error) => Err(Error::Startup(format!("model setup: {error}"))),
    }
}

/// `channels` is what the API lists (the guild cache, or nothing without
/// a guild reader).
pub async fn compose(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    channels: Arc<dyn ChannelList>,
    health: LiveHealth,
) -> Result<Composition, Error> {
    compose_with(
        config,
        store,
        channels,
        health,
        None,
        Arc::new(clock::system_now),
    )
    .await
}

/// [`compose`] on `clock` (the writer's, the API's and both sign-in realms'
/// sessions); a `shared` model stack (built once by the caller from the same
/// config) replaces building one, and its listing and refresh stay the caller's.
pub(in crate::runtime::serve) async fn compose_with(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    channels: Arc<dyn ChannelList>,
    health: LiveHealth,
    shared: Option<Arc<ModelStack>>,
    clock: Clock,
) -> Result<Composition, Error> {
    let catalog = load_catalog(&config.files.catalog_file).map_err(file_error)?;
    let knowledge = config
        .files
        .knowledge_dir
        .as_deref()
        .map(load_knowledge_dir)
        .transpose()
        .map_err(file_error)?
        .map(Arc::new);
    let mut settings = settings::load(&store, &config.seeds).await?;
    // Loaded above, so a read failure here is a transient store error.
    let stored = store
        .settings_rows()
        .await
        .map_err(|_| Error::Startup("runtime settings could not be read".into()))?;
    // An unsaved seed was applied by `load`; refuse it before anything uses it.
    if !stored.contains_key(kanade_domain::settings::keys::RUN_LENGTHS) {
        validate_run_lengths_seed(config.seeds.run_lengths.as_ref(), &catalog)?;
    }
    let sources = model_report::seed_roles(&mut settings, &config.models, &stored);
    let (models, model_tasks) = match shared {
        Some(stack) => (Some(stack), ModelTasks::default()),
        None => {
            let models = model_stack(&config.models, &settings, config.guild.guild_id)?;
            let tasks =
                model_report::start(models.as_ref(), sources, settings.models.context.clone());
            (models, tasks)
        }
    };
    let personas = load_personas(
        &config.files.persona_dir,
        settings::persona(&settings)?.as_ref(),
    )
    .map_err(file_error)?;
    crate::runtime::serve::persona_log::persona_selected(&personas.snapshot);
    let policy = settings.schedule_policy(config.runtime.timezone);

    let access = Arc::new(access(&config.guild));
    let persona_store = Arc::new(PersonaStore::new(personas.snapshot));
    let desk = Arc::new(ConfigDesk::new(ConfigInputs {
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
    }));
    let catalog = Arc::new(catalog);
    let writer: Arc<dyn Writer> = Arc::new(SchedulerWriter::new(
        SchedulerService::new(store.clone(), RandomIds, ApiClock(clock.clone()))
            .with_attendance(policy.attendance)
            .with_run_ends(desk.run_ends(Arc::clone(&catalog), policy.clone())),
    ));
    // Read once: the module's routes answer 404 until a restart turns it on.
    let characters = config
        .modules
        .roster
        .then(|| store.clone() as Arc<dyn CharacterRoster>);
    let health = Arc::new(health);
    #[cfg(feature = "web")]
    let admin = super::web::api::admin(
        config,
        store,
        channels,
        super::web::api::Shared {
            access: &access,
            desk: &desk,
            models: models.as_ref(),
            writer: &writer,
            policy: &policy,
            catalog: &catalog,
            knowledge: knowledge.as_ref(),
            characters: &characters,
            health: &health,
            clock: &clock,
        },
    )
    .await?;
    // Only the API lists channels and reads the store directly.
    #[cfg(not(feature = "web"))]
    let _ = (store, channels);
    Ok(Composition {
        #[cfg(feature = "web")]
        admin,
        access,
        settings,
        personas: persona_store,
        models,
        knowledge,
        model_tasks,
        catalog,
        policy,
        config: desk,
        writer,
        clock,
        characters,
        health,
        character_data: None,
    })
}
