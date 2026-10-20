//! Admin Setup (`GET /api/admin/setup`): a read-only checklist of what this
//! deployment has set up, put together from facts the server already holds
//! (health, the access report, Config, module health, the backup directory).
//! Nothing is written. A fact this server cannot read makes its item
//! `missing`, `off` or `warn`, never a failed page, and only presence is
//! reported: no secret, path or URL of the deployment is ever sent.

use std::{future::Future, sync::Arc, time::Duration};

use axum::{Json, Router, extract, routing::get};
use twilight_model::oauth::ApplicationFlags;

use super::{
    config::{ConfigDesk, REPORTED, lacking},
    history::backups,
};
use crate::{
    api::{
        auth::AdminSession,
        dto::{
            iso_instant,
            setup::{
                PrivilegedIntent, SetupItem, SetupItemId as Id, SetupReason as Reason,
                SetupSource as Source, SetupState as State, SetupView,
            },
        },
        listeners::Site,
        state::{ApiState, SetupFacts},
    },
    bot::transport::ApplicationInfo,
    domain::settings::{RuntimeSettings, keys},
    infrastructure::{
        character_data::{Health, NexonState},
        llm::governor::Role,
        scouter::ScouterState,
    },
};

pub fn routes() -> Router<Arc<Site>> {
    Router::new().route("/api/admin/setup", get(read))
}

/// Each outside read (Discord's application, the model catalog, module
/// health, the backup directory) ends by then, so the page never hangs; a
/// read cut off reports its item as not answered.
const DEADLINE: Duration = Duration::from_secs(5);

async fn bounded<T>(read: impl Future<Output = T>) -> Option<T> {
    tokio::time::timeout(DEADLINE, read).await.ok()
}

/// Facts read for one answer; `None` where this server has none.
struct Facts<'a> {
    state: Option<&'a ApiState>,
    desk: Option<&'a ConfigDesk>,
    settings: Option<RuntimeSettings>,
    saved: Vec<&'static str>,
    /// `/healthz`'s `discord`, when health is composed.
    gateway: Option<&'static str>,
    application: Option<ApplicationInfo>,
    /// `None`: no gateway configured, or the read was cut off.
    reachable: Option<bool>,
    character_data: Option<Health>,
    latest_backup: Option<String>,
}

async fn read(extract::State(site): extract::State<Arc<Site>>, _: AdminSession) -> Json<SetupView> {
    let facts = facts(&site).await;
    Json(SetupView::new(items(&site, &facts)))
}

async fn facts(site: &Site) -> Facts<'_> {
    let state = site.state.as_deref();
    let desk = state.and_then(|state| state.config.as_deref());
    let settings = match desk {
        Some(desk) => Some(desk.settings().await),
        None => None,
    };
    let (saved, gateway, application, catalog, character_data, latest_backup) = tokio::join!(
        async {
            match desk {
                // Only the source labels depend on it, so a slow store read degrades to "environment".
                Some(desk) => bounded(desk.saved_keys()).await.unwrap_or_default(),
                None => Vec::new(),
            }
        },
        async {
            match &site.health {
                Some(probe) => bounded(probe.health()).await.map(|health| health.discord),
                None => None,
            }
        },
        async {
            match state.and_then(|state| state.application.as_ref()) {
                Some(port) => bounded(port()).await.flatten(),
                None => None,
            }
        },
        async {
            match desk.filter(|desk| desk.gateway_configured()) {
                Some(desk) => bounded(desk.catalog()).await.map(|read| read.reachable),
                None => None,
            }
        },
        async {
            match state.and_then(|state| state.character_data.as_ref()) {
                Some(module) => bounded(module.health()).await,
                None => None,
            }
        },
        async {
            match state.and_then(|state| state.backups.dir.clone()) {
                Some(dir) => bounded(backups::latest(dir)).await.flatten(),
                None => None,
            }
        },
    );
    Facts {
        state,
        desk,
        settings,
        saved,
        gateway,
        application,
        reachable: catalog,
        character_data,
        latest_backup: latest_backup.map(iso_instant),
    }
}

fn items(site: &Site, facts: &Facts<'_>) -> Vec<SetupItem> {
    let setup = facts
        .state
        .map_or_else(SetupFacts::default, |state| state.setup);
    Id::ALL
        .iter()
        .map(|&id| match id {
            Id::DiscordGateway => gateway(facts),
            Id::DiscordIntents => intents(facts.application),
            Id::BotAccess => access(facts),
            Id::DigestChannel => digest(facts),
            Id::WatchedChannels => watched(facts),
            Id::ModelGateway => model_gateway(facts),
            Id::ExtractionModel => extraction(facts),
            Id::Chat => chat(facts),
            Id::Persona => persona(facts),
            Id::AdminSignIn => admin_sign_in(site),
            Id::PublicPortal => public_portal(&setup, facts.settings.as_ref()),
            Id::Roster => module(id, setup.modules.roster),
            Id::CharacterData => character_data(&setup, facts.character_data.as_ref()),
            Id::Guides => module(id, setup.modules.guides),
            Id::Scouter => scouter(&setup, facts),
            Id::Backups => backup(&setup, facts),
        })
        .collect()
}

/// Ready from health; without a health probe (offline), from whether the
/// guild is loaded.
fn gateway(facts: &Facts<'_>) -> SetupItem {
    let missing = |reason| SetupItem::new(Id::DiscordGateway, State::Missing).because(reason);
    match facts.gateway {
        Some("ready") => SetupItem::new(Id::DiscordGateway, State::Ok),
        Some("connecting" | "disconnected") => missing(Reason::Connecting),
        Some("closed") => missing(Reason::GatewayClosed),
        Some(_) => missing(Reason::GatewayOff),
        None if facts.state.is_some_and(|state| state.channels.connected()) => {
            SetupItem::new(Id::DiscordGateway, State::Ok)
        }
        None => missing(Reason::GatewayOff),
    }
}

/// Discord's Application Flags: each intent in its 100+ servers form or the
/// under-100 form the Bot page switches on.
fn intents(application: Option<ApplicationInfo>) -> SetupItem {
    let Some(application) = application else {
        return SetupItem::new(Id::DiscordIntents, State::Warn).because(Reason::Unavailable);
    };
    let has = |flags: ApplicationFlags| application.flags.intersects(flags);
    let missing: Vec<PrivilegedIntent> = [
        (
            PrivilegedIntent::ServerMembers,
            ApplicationFlags::GATEWAY_GUILD_MEMBERS
                | ApplicationFlags::GATEWAY_GUILD_MEMBERS_LIMITED,
        ),
        (
            PrivilegedIntent::MessageContent,
            ApplicationFlags::GATEWAY_MESSAGE_CONTENT
                | ApplicationFlags::GATEWAY_MESSAGE_CONTENT_LIMITED,
        ),
    ]
    .into_iter()
    .filter(|(_, flags)| !has(*flags))
    .map(|(intent, _)| intent)
    .collect();
    let state = if missing.is_empty() {
        State::Ok
    } else {
        State::Missing
    };
    SetupItem {
        missing_intents: Some(missing),
        ..SetupItem::new(Id::DiscordIntents, state)
    }
}

/// The access report's six grants over every watched and digest channel,
/// with the invite link asking for exactly those six.
fn access(facts: &Facts<'_>) -> SetupItem {
    let digest = facts
        .settings
        .as_ref()
        .and_then(|settings| settings.posting.channel_id.as_deref());
    let short = facts
        .state
        .and_then(|state| lacking(state.channels.as_ref(), digest));
    let item = match short {
        None => SetupItem::new(Id::BotAccess, State::Missing).because(Reason::NotConnected),
        Some(0) => SetupItem::new(Id::BotAccess, State::Ok),
        Some(_) => SetupItem::new(Id::BotAccess, State::Missing),
    };
    SetupItem {
        channels_lacking: short,
        invite_url: facts.application.map(|application| {
            format!(
                "https://discord.com/oauth2/authorize?client_id={}&scope=bot+applications.commands&permissions={}",
                application.id,
                REPORTED.bits()
            )
        }),
        ..item
    }
}

fn seeded(facts: &Facts<'_>, key: &str) -> Source {
    if facts.saved.contains(&key) {
        Source::Saved
    } else {
        Source::Environment
    }
}

fn digest(facts: &Facts<'_>) -> SetupItem {
    let Some(settings) = &facts.settings else {
        return SetupItem::new(Id::DigestChannel, State::Missing);
    };
    let saved = facts.saved.contains(&keys::POST_CHANNEL);
    match &settings.posting.channel_id {
        Some(id) => SetupItem {
            channel_id: Some(id.clone()),
            ..SetupItem::new(Id::DigestChannel, State::Ok)
                .from_source(seeded(facts, keys::POST_CHANNEL))
        },
        // Cleared by a save: the saved row now says "none".
        None if saved => {
            SetupItem::new(Id::DigestChannel, State::Missing).from_source(Source::Saved)
        }
        None => SetupItem::new(Id::DigestChannel, State::Missing),
    }
}

fn watched(facts: &Facts<'_>) -> SetupItem {
    let Some(settings) = &facts.settings else {
        return SetupItem::new(Id::WatchedChannels, State::Missing);
    };
    let watching = &settings.watching;
    let count = watching.channel_ids.len() + watching.category_ids.len();
    let state = if count == 0 {
        State::Missing
    } else {
        State::Ok
    };
    let source = if [keys::WATCHED_CHANNELS, keys::WATCHED_CATEGORIES]
        .iter()
        .any(|key| facts.saved.contains(key))
    {
        Source::Saved
    } else {
        Source::Environment
    };
    SetupItem {
        count: Some(u32::try_from(count).unwrap_or(u32::MAX)),
        ..SetupItem::new(Id::WatchedChannels, state).from_source(source)
    }
}

fn model_gateway(facts: &Facts<'_>) -> SetupItem {
    match facts.reachable {
        Some(true) => SetupItem::new(Id::ModelGateway, State::Ok),
        Some(false) => SetupItem::new(Id::ModelGateway, State::Warn).because(Reason::Unreachable),
        // Configured, but the listing did not finish in time.
        None if facts.desk.is_some_and(ConfigDesk::gateway_configured) => {
            SetupItem::new(Id::ModelGateway, State::Warn).because(Reason::Unreachable)
        }
        None => SetupItem::new(Id::ModelGateway, State::Missing),
    }
}

/// Routed: the running stack serves the extraction role.
fn extraction(facts: &Facts<'_>) -> SetupItem {
    let (Some(desk), Some(settings)) = (facts.desk, &facts.settings) else {
        return SetupItem::new(Id::ExtractionModel, State::Missing);
    };
    if desk.running().contains_key(&Role::Extraction) {
        SetupItem::new(Id::ExtractionModel, State::Ok)
    } else if settings.models.extraction.alias.is_some() {
        SetupItem::new(Id::ExtractionModel, State::Warn).because(Reason::NotRunning)
    } else {
        SetupItem::new(Id::ExtractionModel, State::Missing)
    }
}

fn chat(facts: &Facts<'_>) -> SetupItem {
    let (Some(desk), Some(settings)) = (facts.desk, &facts.settings) else {
        return SetupItem::new(Id::Chat, State::Off);
    };
    if !settings.chatbot.enabled {
        return SetupItem::new(Id::Chat, State::Off);
    }
    let missing = desk.missing_env(settings);
    if missing.is_empty() {
        SetupItem::new(Id::Chat, State::Ok)
    } else {
        SetupItem {
            missing_env: Some(missing),
            ..SetupItem::new(Id::Chat, State::Warn).because(Reason::EnvMissing)
        }
    }
}

fn persona(facts: &Facts<'_>) -> SetupItem {
    match &facts.settings {
        Some(settings) if !settings.persona.active.is_empty() => {
            SetupItem::new(Id::Persona, State::Ok).from_source(Source::Saved)
        }
        _ => SetupItem::new(Id::Persona, State::Missing),
    }
}

/// Discord sign-in, or Tailscale logins behind the authenticated edge;
/// break-glass alone does not count.
fn admin_sign_in(site: &Site) -> SetupItem {
    let configured = site.auth.as_ref().is_some_and(|auth| {
        auth.discord().is_some() || (auth.tailscale_enabled() && site.edge_secret.is_some())
    });
    let state = if configured {
        State::Ok
    } else {
        State::Missing
    };
    SetupItem::new(Id::AdminSignIn, state).from_source(Source::Environment)
}

fn public_portal(setup: &SetupFacts, settings: Option<&RuntimeSettings>) -> SetupItem {
    if !setup.public_listener {
        return SetupItem::new(Id::PublicPortal, State::Off).from_source(Source::Environment);
    }
    let item = if setup.member_sign_in {
        SetupItem::new(Id::PublicPortal, State::Ok)
    } else {
        SetupItem::new(Id::PublicPortal, State::Warn).because(Reason::MemberSignInMissing)
    };
    SetupItem {
        open: settings.map(|settings| settings.self_service.public_portal),
        ..item.from_source(Source::Environment)
    }
}

fn module(id: Id, on: bool) -> SetupItem {
    let state = if on { State::Ok } else { State::Off };
    SetupItem::new(id, state).from_source(Source::Environment)
}

fn character_data(setup: &SetupFacts, health: Option<&Health>) -> SetupItem {
    if !setup.modules.character_data {
        return module(Id::CharacterData, false);
    }
    let item = match health.map(|health| health.nexon) {
        Some(NexonState::Ok) => SetupItem::new(Id::CharacterData, State::Ok),
        Some(NexonState::NoKey) => warn(Id::CharacterData, Reason::NexonNoKey),
        Some(NexonState::KeyRefused) => warn(Id::CharacterData, Reason::NexonKeyRefused),
        Some(NexonState::Backoff) => warn(Id::CharacterData, Reason::NexonBackoff),
        None => warn(Id::CharacterData, Reason::Unavailable),
    };
    item.from_source(Source::Environment)
}

fn warn(id: Id, reason: Reason) -> SetupItem {
    SetupItem::new(id, State::Warn).because(reason)
}

/// Off without module `character_data` or `KANADE_SCOUTER_URL`.
fn scouter(setup: &SetupFacts, facts: &Facts<'_>) -> SetupItem {
    let configured = setup.modules.character_data
        && facts
            .state
            .and_then(|state| state.character_data.as_ref())
            .is_some_and(|module| module.estimates().is_some());
    if !configured {
        return module(Id::Scouter, false);
    }
    let item = match facts
        .character_data
        .as_ref()
        .map(|health| health.scouter.state)
    {
        Some(ScouterState::Ok) => SetupItem::new(Id::Scouter, State::Ok),
        Some(ScouterState::Off) => SetupItem::new(Id::Scouter, State::Off),
        Some(ScouterState::Unreachable) => warn(Id::Scouter, Reason::Unreachable),
        Some(ScouterState::VerificationRequired) => warn(Id::Scouter, Reason::VerificationRequired),
        None => warn(Id::Scouter, Reason::Unavailable),
    };
    item.from_source(Source::Environment)
}

fn backup(setup: &SetupFacts, facts: &Facts<'_>) -> SetupItem {
    let Some(state) = facts.state else {
        return SetupItem::new(Id::Backups, State::Missing);
    };
    if state.backups.dir.is_none() {
        return SetupItem::new(Id::Backups, State::Missing).from_source(Source::Environment);
    }
    SetupItem {
        latest_backup_at: facts.latest_backup.clone(),
        recipients: Some(setup.backup_recipients),
        ..SetupItem::new(Id::Backups, State::Ok).from_source(Source::Environment)
    }
}
