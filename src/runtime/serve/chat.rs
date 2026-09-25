//! The chat pilot in live serve: the model side of the driver
//! ([`ServeAnswerer`]: live settings, persona, members, the chat model route,
//! the scheduler for proposals and the card desk for their cards), the
//! driver start (withheld ids reloaded before any admission) and its stop.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::{
    api::{admin::config::SettingsChanged, auth::Clock, state::GuildAccess},
    bot::{
        chat_feed::{ChatFeed, DiscordSurface, StaffFn},
        commands::{ChatAllowance, Invoker},
        delivery::FixedClock,
        guild_cache::GuildCache,
        roster::LiveRoster,
    },
    chat::{
        answer::{AnswerDeps, Generation, GuildView, answer},
        driver::{Answerer, Asked, ChatDriver, ChatHandle, DriverConfig, Job, Prepared, Setup},
        gate::{ChannelDirectory, PilotSettings},
        persona::{PersonaStore, ProfileId, ProfileQuery},
        pilot::{AllowanceSnapshot, StormAlert},
        tools::propose::Proposer,
    },
    domain::{
        catalog::BossTable,
        ids::RandomIds,
        members::{MemberStore, Roster},
        model_log::{ChatInteraction, ModelLogStore},
        schedule::SchedulePolicy,
        scheduler::SchedulerService,
        settings::RuntimeSettings,
    },
    infrastructure::{
        llm::{
            governor::Role,
            identity::{self, Passthrough},
            setup::ModelStack,
        },
        store::SqliteStore,
    },
    runtime::{error::Error, logging},
};

use super::chat_cards::{self as cards, ChatDesk};
use super::discord::GatewayTransport;

impl ChatAllowance for ChatHandle {
    fn snapshot(&self) -> AllowanceSnapshot {
        self.allowance()
    }
}

/// The model side of live chat.
pub struct ServeAnswerer<T> {
    pub store: Arc<SqliteStore>,
    pub models: Option<Arc<ModelStack>>,
    pub personas: Arc<PersonaStore>,
    pub settings: watch::Receiver<SettingsChanged>,
    pub cache: Arc<GuildCache>,
    pub catalog: Arc<BossTable>,
    pub policy: SchedulePolicy,
    pub guild_id: String,
    pub pilot_role: Option<String>,
    pub clock: Clock,
    pub desk: Arc<ChatDesk<T>>,
}

impl<T> ServeAnswerer<T> {
    fn settings(&self) -> Arc<RuntimeSettings> {
        Arc::clone(&self.settings.borrow().settings)
    }

    fn pilot(&self, settings: &RuntimeSettings) -> PilotSettings {
        PilotSettings {
            guild_id: self.guild_id.clone(),
            channel_ids: Vec::new(),
            category_ids: settings.chatbot.category_ids.clone(),
            role_id: self.pilot_role.clone(),
        }
    }

    fn route(&self) -> Option<(&Arc<ModelStack>, String)> {
        let stack = self.models.as_ref()?;
        let route = stack.governor.route(Role::Chat)?;
        Some((stack, route.alias))
    }
}

fn identity_roster(members: &[crate::domain::members::MemberProfile]) -> Vec<identity::Member> {
    members
        .iter()
        .map(|profile| identity::Member {
            user_id: profile.member.user_id.clone(),
            display_name: profile.member.display_name.clone().unwrap_or_default(),
            nickname: profile.member.nickname.clone(),
            aliases: profile.aliases.clone(),
        })
        .collect()
}

impl<T: GatewayTransport> Answerer for ServeAnswerer<T> {
    fn setup(&self) -> Setup {
        let settings = self.settings();
        let chat = &settings.chatbot;
        let model = self.route().map(|(_, alias)| alias);
        let persona = self.personas.pin().active().is_some();
        Setup {
            enabled: chat.enabled,
            ready: model.is_some() && persona,
            pilot: self.pilot(&settings),
            member_rate: (
                chat.member_rate.count as usize,
                f64::from(chat.member_rate.window_s),
            ),
            pool_rate: (
                chat.guild_rate.count as usize,
                f64::from(chat.guild_rate.window_s),
            ),
            model: model.unwrap_or_default(),
            now: (self.clock)(),
        }
    }

    fn channels(&self) -> &(dyn ChannelDirectory + Send + Sync) {
        &*self.cache
    }

    async fn prepare(&self, asked: &Asked) -> Option<Prepared> {
        let (stack, model) = self.route()?;
        let members = match self.store.list_members().await {
            Ok(members) => members,
            Err(_) => {
                logging::event("WARN", "chat_members_unreadable", json!({}));
                Vec::new()
            }
        };
        let snapshot = self.personas.pin();
        let active = snapshot.active()?;
        // The member's saved reply style, when it is still readable.
        let selectable: BTreeSet<ProfileId> = active.profiles.readable.keys().cloned().collect();
        let saved = members
            .iter()
            .find(|profile| profile.member.user_id == asked.message.author_id)
            .and_then(|profile| profile.reply_style.as_deref())
            .and_then(|style| ProfileId::parse(style).ok());
        let query = ProfileQuery {
            member_roles: &[],
            role_assignments: &[],
            saved_selection: saved.as_ref(),
            selectable: &selectable,
        };
        let persona = snapshot.resolve(&query)?.compile();
        let bundle = &active.bundle.value;
        let persona_key = format!("{}\n{}", bundle.id, bundle.identity);
        let mut roster = Roster::new();
        for profile in &members {
            roster.upsert(profile.member.clone());
        }
        let settings = self.settings();
        Some(Prepared {
            persona,
            persona_key,
            directory: Arc::new(roster),
            members,
            pilot: self.pilot(&settings),
            model,
            reasoning: stack.effort(Role::Chat),
            now: (self.clock)(),
            zone: self.policy.zone(),
            reset: (self.policy.reset_weekday, self.policy.reset_time),
            bot_names: self.cache.self_names(),
        })
    }

    async fn answer(&self, job: Job<'_>) -> Generation {
        let Some((stack, _)) = self.route() else {
            return Generation::default();
        };
        let prepared = job.prepared;
        let roster = identity_roster(&prepared.members);
        let members: Vec<_> = prepared
            .members
            .iter()
            .map(|profile| profile.member.clone())
            .collect();
        let deps = AnswerDeps {
            client: &stack.client,
            codec: &Passthrough,
            roster: &roster,
        };
        let guild = GuildView {
            members: &members,
            directory: &*prepared.directory,
            catalog: &self.catalog,
            channels: &*self.cache,
            pilot: &prepared.pilot,
            zone: prepared.zone,
            reset_weekday: prepared.reset.0,
            reset_time: prepared.reset.1,
            guides: None,
        };
        let mut service =
            SchedulerService::new(Arc::clone(&self.store), RandomIds, FixedClock(prepared.now))
                .with_attendance(self.policy.attendance);
        let mut proposer = Proposer {
            service: &mut service,
            policy: &self.policy,
        };
        let ports = cards::ChatCards {
            desk: &self.desk,
            store: &self.store,
            zone: prepared.zone,
            cancelled: job.cancelled,
        };
        answer(&deps, job.question, &guild, &mut proposer, &ports).await
    }

    async fn record(&self, row: ChatInteraction) {
        if self.store.record_chat(row).await.is_err() {
            logging::event("WARN", "chat_log_failed", json!({}));
        }
    }

    fn storm(&self, alert: &StormAlert) {
        logging::event(
            "WARN",
            "admin_alert",
            json!({
                "kind": "clean_retry_storm",
                "retries": alert.retries,
                "window_s": alert.window_s,
            }),
        );
    }
}

/// Staff by the command gates' rule: the admin role, Administrator (from the
/// roster) or the guild owner.
pub fn staff(roster: Arc<LiveRoster>, access: Arc<GuildAccess>) -> StaffFn {
    Arc::new(move |user_id, roles| {
        let is_guild_admin = roster
            .profile(&user_id.get().to_string())
            .is_some_and(|profile| profile.is_guild_admin);
        let invoker = Invoker {
            user_id,
            roles: roles.to_vec(),
            is_guild_admin,
        };
        access.policy.is_staff(&invoker, access.owner())
    })
}

type Stop = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// The running chat side, stopped by the Discord side after the gateway.
pub struct ChatRuntime {
    stop: Option<Stop>,
    overrides: JoinHandle<()>,
}

impl ChatRuntime {
    /// Refund waiting questions, let running ones finish within the grace,
    /// cut the rest; each concludes and logs.
    pub async fn stop(&mut self) {
        self.overrides.abort();
        if let Some(stop) = self.stop.take() {
            stop().await;
        }
    }
}

/// Everything the chat side is started from.
pub struct ChatInputs<T> {
    pub config: DriverConfig,
    pub answerer: ServeAnswerer<T>,
    pub transport: Arc<T>,
    pub handle: Option<Arc<ChatHandle>>,
    pub roster: Arc<LiveRoster>,
    pub access: Arc<GuildAccess>,
}

/// Start the driver (withheld ids reloaded first) and the override refresh
/// on every settings change; the feed goes to the gateway handler.
pub async fn start<T: GatewayTransport>(
    inputs: ChatInputs<T>,
) -> Result<(ChatFeed, ChatRuntime), Error> {
    let ChatInputs {
        config,
        answerer,
        transport,
        handle,
        roster,
        access,
    } = inputs;
    let store = Arc::clone(&answerer.store);
    let cache = Arc::clone(&answerer.cache);
    let mut changes = answerer.settings.clone();
    let base = Instant::now();
    let driver = ChatDriver::start(
        config,
        answerer,
        DiscordSurface(transport),
        &*store,
        Arc::new(move || base.elapsed().as_secs_f64()),
    )
    .await
    .map_err(|error| Error::Startup(format!("chat: {error}")))?;
    if let Some(handle) = handle {
        handle.set(Arc::new(driver.clone()));
    }
    let refresh = driver.clone();
    let overrides = tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            if let Ok(rows) = store.allowance_overrides().await {
                refresh.set_overrides(rows);
            }
        }
    });
    let feed = ChatFeed::new(Arc::new(driver.clone()), cache, staff(roster, access));
    let stop: Stop = Box::new(move || Box::pin(async move { driver.stop().await }));
    Ok((
        feed,
        ChatRuntime {
            stop: Some(stop),
            overrides,
        },
    ))
}
