//! The admin API over the shared composition: its state, sign-in and the
//! staff gate.

use std::sync::Arc;

use crate::{
    api::{
        admin::limits::LimitsDesk,
        auth::{
            self, AdminAuth,
            audit::StoreAudit,
            member::{MemberAuth, PortalOpen, StoreEligibility},
            staff::{GuildStaffGate, StoreGuildMembers},
        },
        events::Hub,
        server::LiveAdmin,
        state::{ApiState, BackupDir, SetupFacts},
    },
    app::{
        clock::Clock,
        config::ConfigDesk,
        guild::{ChannelList, GuildAccess},
        read::CharacterRoster,
        write::Writer,
    },
    discord::avatars::AvatarCache,
    infrastructure::{files::KnowledgeDir, llm::setup::ModelStack, store::SqliteStore},
    runtime::{config::ServeConfig, error::Error, serve::health::LiveHealth},
};
use kanade_domain::{catalog::BossTable, schedule::SchedulePolicy};

#[cfg(any(test, feature = "test-support"))]
use crate::runtime::serve::composition::{Composition, compose_with};

/// What the API shares with the rest of serve's composition.
pub(in crate::runtime::serve) struct Shared<'a> {
    pub access: &'a Arc<GuildAccess>,
    pub desk: &'a Arc<ConfigDesk>,
    pub models: Option<&'a Arc<ModelStack>>,
    pub writer: &'a Arc<dyn Writer>,
    pub policy: &'a SchedulePolicy,
    pub catalog: &'a Arc<BossTable>,
    pub knowledge: Option<&'a Arc<KnowledgeDir>>,
    pub characters: &'a Option<Arc<dyn CharacterRoster>>,
    pub health: &'a Arc<LiveHealth>,
    pub clock: &'a Clock,
}

/// The admin state over `store` and `shared`, both sign-in realms (the
/// member realm only with a public listener) and the staff gate. The
/// Discord side attaches its ports to the state before HTTP serves.
pub(in crate::runtime::serve) async fn admin(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    channels: Arc<dyn ChannelList>,
    shared: Shared<'_>,
) -> Result<LiveAdmin, Error> {
    let Shared {
        access,
        desk,
        models,
        writer,
        policy,
        catalog,
        knowledge,
        characters,
        health,
        clock,
    } = shared;
    let staff = GuildStaffGate::new(
        access.policy.clone(),
        Arc::new(StoreGuildMembers::new(store.clone(), access.clone())),
    );
    let auth: AdminAuth =
        auth::from_settings(&config.runtime.admin_auth, store.clone(), Arc::new(staff))?
            .with_audit(Arc::new(StoreAudit::new(store.clone())))
            .with_clock(clock.clone());
    // One portrait cache for both origins.
    let avatars = Arc::new(AvatarCache::discord(
        config.runtime.http.identity_dir.as_deref(),
    ));
    let member = match config.runtime.public_bind {
        Some(_) => Some(Arc::new(
            member_realm(config, &store, desk, &avatars)?.with_clock(clock.clone()),
        )),
        None => None,
    };

    // Fixed for the process: the store was migrated when it opened.
    let schema_version = store
        .schema_version()
        .await
        .map_err(|_| Error::Startup("store schema version could not be read".into()))?;
    // Every write path (portal, Discord, extractor, tick) commits through this
    // store, so its write hook is the one place change hints come from.
    let events = Arc::new(Hub::default());
    store.observe_writes(events.observer());
    let state = ApiState {
        store,
        writer: Arc::clone(writer),
        policy: policy.clone(),
        catalog: Arc::clone(catalog),
        channels,
        access: access.clone(),
        knowledge_dir: knowledge.map(|dir| dir.path.clone()),
        guild_id: Some(config.guild.guild_id.to_string()),
        clock: clock.clone(),
        rescans: None,
        config: Some(Arc::clone(desk)),
        chat: Some(health.chat()),
        model_limits: models.map(|stack| {
            let governor = Arc::clone(&stack.governor);
            Arc::new(move |at| governor.snapshot(at)) as crate::app::ports::ModelLimits
        }),
        limits: Arc::new(LimitsDesk::default()),
        proposal_refresh: None,
        decline_retraction: None,
        digest_post: None,
        header_rewrite: None,
        // Serve's Discord side attaches it with the transport.
        application: None,
        backups: BackupDir {
            dir: config.backup_dir.clone(),
            schema_version,
        },
        setup: SetupFacts {
            public_listener: config.runtime.public_bind.is_some()
                && config.runtime.http.public_host.is_some(),
            member_sign_in: member
                .as_ref()
                .is_some_and(|member| member.discord().is_some()),
            modules: config.modules,
            backup_recipients: match &config.backup_recipients_file {
                Some(path) => crate::runtime::secrets::present(
                    path,
                    crate::runtime::config::BACKUP_RECIPIENTS_LABEL,
                )?,
                None => false,
            },
        },
        setup_secrets: super::setup_secrets::desk(config)?,
        avatars: Some(avatars),
        events,
        presence: Default::default(),
        marks: Default::default(),
        characters: characters.clone(),
        character_data: None,
        // Module `guides`: editable without the bot; serve's Discord side
        // replaces the port with its publisher.
        guides: config.modules.guides.then(|| {
            Arc::new(crate::app::ports::OfflineGuides) as Arc<dyn crate::app::ports::GuidePort>
        }),
    };
    Ok(LiveAdmin {
        auth: Arc::new(auth),
        state: Arc::new(state),
        health: Arc::clone(health) as _,
        member,
    })
}

/// Test support: what [`compose_for_tests`] puts in place of serve's
/// outside world.
#[cfg(any(test, feature = "test-support"))]
pub struct Overrides {
    /// The wall clock of the writer, the API, both sign-in realms and module
    /// `character_data`.
    pub clock: Clock,
    /// The Discord API behind both sign-in realms the config sets up.
    pub discord: Option<Arc<dyn crate::api::auth::discord::DiscordApi>>,
    /// Both realms' authorization page (the fake Discord's).
    pub authorize_url: Option<String>,
    /// Governor snapshots in place of the model stack's.
    pub model_limits: Option<crate::app::ports::ModelLimits>,
    /// A loopback fake NEXON Open API (plain HTTP) for module `character_data`.
    pub nexon: Option<std::net::SocketAddr>,
    /// Run the background estimate refresh on serve's schedule (10 minutes
    /// after start, then hourly); off, nothing looks up on its own.
    pub estimate_refresh: bool,
    /// Setup's secret checks in place of Discord and the gateway (only with
    /// a saved secrets directory).
    pub secret_checks: Option<Arc<dyn crate::api::state::SecretChecks>>,
    /// Setup's "Restart now" in place of a real shutdown.
    pub restart: Option<crate::api::state::RestartPort>,
}

/// Test support: the composition and its running module `character_data`
/// (stop it before closing the store).
#[cfg(any(test, feature = "test-support"))]
pub struct TestComposition {
    pub composition: Composition,
    pub character_data: Option<crate::runtime::serve::CharacterDataModule>,
}

/// Test support: compose as serve does without the gateway (no guild cache,
/// then module `character_data` on the same clock), with `overrides` in place
/// of the system clock, Discord, the model governor and NEXON.
#[cfg(any(test, feature = "test-support"))]
pub async fn compose_for_tests(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    overrides: Overrides,
) -> Result<TestComposition, Error> {
    use crate::{
        api::auth::discord::DiscordLogin, app::guild::StaticChannels,
        infrastructure::nexon::NexonClient,
    };

    let Overrides {
        clock,
        discord,
        authorize_url,
        model_limits,
        nexon,
        estimate_refresh,
        secret_checks,
        restart,
    } = overrides;
    let mut composition = compose_with(
        config,
        store.clone(),
        Arc::new(StaticChannels(Vec::new())),
        LiveHealth::new(store.clone()),
        None,
        Arc::clone(&clock),
    )
    .await?;
    // Compose returned the only handles.
    let unshared = || Error::Startup("the composition is already shared".into());
    let fake = |mut login: DiscordLogin| {
        if let Some(api) = &discord {
            login.api = Arc::clone(api);
        }
        match &authorize_url {
            Some(url) => login.with_authorize_url(url.clone()),
            None => login,
        }
    };
    let admin = &mut composition.admin;
    Arc::get_mut(&mut admin.auth)
        .ok_or_else(unshared)?
        .map_discord(fake);
    if let Some(member) = &mut admin.member {
        Arc::get_mut(member).ok_or_else(unshared)?.map_discord(fake);
    }
    if model_limits.is_some() {
        Arc::get_mut(&mut admin.state)
            .ok_or_else(unshared)?
            .model_limits = model_limits;
    }
    if let Some(checks) = secret_checks {
        let state = Arc::get_mut(&mut admin.state).ok_or_else(unshared)?;
        if let Some(files) = state.setup_secrets.as_ref().map(|desk| desk.files.clone()) {
            state.setup_secrets = Some(Arc::new(crate::api::state::SecretDesk::new(
                files, checks, restart,
            )));
        }
    }
    let character_data = crate::runtime::serve::character_data::start_with(
        config,
        &store,
        &mut composition,
        clock,
        |key| match nexon {
            Some(address) => Some(NexonClient::plain(address, key)),
            None => NexonClient::new(key),
        },
        estimate_refresh.then_some(crate::runtime::serve::character_data::REFRESH),
    )
    .await?;
    Ok(TestComposition {
        composition,
        character_data,
    })
}

/// The public portal's member realm: eligibility from the stored roster and
/// the open switch read live from the admin Config (`self_service.public_portal`).
fn member_realm(
    config: &ServeConfig,
    store: &Arc<SqliteStore>,
    desk: &ConfigDesk,
    avatars: &Arc<AvatarCache>,
) -> Result<MemberAuth, Error> {
    let changes = desk.subscribe();
    let open: PortalOpen = Arc::new(move || changes.borrow().settings.self_service.public_portal);
    Ok(auth::member_from_settings(
        &config.runtime.public_auth,
        store.clone(),
        Arc::new(StoreEligibility::new(store.clone())),
        open,
    )?
    .with_portraits(Arc::clone(avatars))
    .with_audit(Arc::new(StoreAudit::new(store.clone()))))
}
