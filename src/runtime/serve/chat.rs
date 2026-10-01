//! The chat pilot in live serve: the model side of the driver
//! ([`ServeAnswerer`]: live settings, persona, members, the chat model route,
//! the scheduler for proposals and the card desk for their cards), the
//! driver start (withheld ids reloaded before any admission) and its stop.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::json;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::{
    api::{
        admin::config::{ConfigDesk, SettingsChanged},
        auth::Clock,
        state::GuildAccess,
    },
    bot::{
        chat_feed::{ChatFeed, DiscordSurface, StaffFn},
        commands::{ChatAllowance, Invoker},
        delivery::FixedClock,
        guild_cache::GuildCache,
        roster::LiveRoster,
    },
    chat::{
        answer::{AnswerDeps, Generation, GuildView, answer},
        driver::{
            Answerer, Asked, ChatDriver, ChatEvent, ChatHandle, DriverConfig, FollowUpRequest, Job,
            Prepared, RejectionFollowUp, Setup,
        },
        gate::{ChannelDirectory, PilotSettings},
        persona::{PersonaStore, ProfileId, ProfileQuery, RoleAssignment, RoleId},
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
            setup::{ModelStack, resolve_context},
        },
        store::SqliteStore,
    },
    runtime::{error::Error, logging},
};

use super::chat_cards::{self as cards, ChatDesk};
use super::chat_log;
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
    pub config: Arc<ConfigDesk>,
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
        // Read once: prompts, logs and requests all use this route, whatever
        // is saved meanwhile.
        let route = self.models.as_ref()?.governor.route(Role::Chat)?;
        let members = match self.store.list_members().await {
            Ok(members) => members,
            Err(_) => {
                logging::event("WARN", "chat_members_unreadable", json!({}));
                Vec::new()
            }
        };
        let settings = self.settings();
        let stack = self.models.as_ref()?;
        let context = resolve_context(
            &settings.models.context,
            &stack.catalog(),
            Role::Chat,
            &route.alias,
        );
        let choices = self.config.profile_choices_for(&settings);
        let snapshot = choices.snapshot.as_deref()?;
        let active = snapshot.active()?;
        // The member's saved reply style, when it is still readable.
        let saved = members
            .iter()
            .find(|profile| profile.member.user_id == asked.message.author_id)
            .and_then(|profile| profile.reply_style.as_deref())
            .and_then(|style| ProfileId::parse(style).ok());
        let member_roles: Vec<_> = asked
            .gate
            .author
            .as_ref()
            .map(|author| author.roles.iter().cloned().map(RoleId::new).collect())
            .unwrap_or_default();
        let role_assignments: Vec<_> = settings
            .persona
            .role_profiles
            .iter()
            .filter_map(|assignment| {
                Some(RoleAssignment {
                    role: RoleId::new(assignment.role_id.clone()),
                    profile: ProfileId::parse(&assignment.profile).ok()?,
                })
            })
            .collect();
        let query = ProfileQuery {
            member_roles: &member_roles,
            role_assignments: &role_assignments,
            saved_selection: saved.as_ref(),
            selectable: &choices.selectable,
        };
        let persona = snapshot.resolve(&query)?.compile();
        let bundle = &active.bundle.value;
        let persona_key = format!("{}\n{}", bundle.id, bundle.identity);
        let mut roster = Roster::new();
        for profile in &members {
            roster.upsert(profile.member.clone());
        }
        Some(Prepared {
            persona,
            catalog: Arc::clone(&self.catalog),
            persona_key,
            directory: Arc::new(roster),
            members,
            pilot: self.pilot(&settings),
            model: route.alias.clone(),
            reasoning: route.effort,
            context_window: context.window as usize,
            max_output_tokens: context.reserve,
            context_source: context.source.as_str(),
            route: Some(route),
            now: (self.clock)(),
            zone: self.policy.zone(),
            reset: (self.policy.reset_weekday, self.policy.reset_time),
            bot_names: self.cache.self_names(),
        })
    }

    async fn owns_rejection(&self, request: &FollowUpRequest) -> bool {
        for source_id in &request.source_ids {
            let Ok(Some(interaction)) = self.store.load_chat(source_id).await else {
                return false;
            };
            if interaction.member_id.as_deref() != Some(request.reactor_id.as_str()) {
                return false;
            }
        }
        true
    }

    async fn answer(&self, job: Job<'_>) -> Generation {
        let Some(stack) = self.models.as_ref() else {
            return Generation::default();
        };
        let prepared = job.prepared;
        let members: Vec<_> = prepared
            .members
            .iter()
            .map(|profile| profile.member.clone())
            .collect();
        let deps = AnswerDeps {
            client: &stack.client,
            route: prepared.route.as_ref(),
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

    fn observe(&self, event: &ChatEvent<'_>) {
        chat_log::observe(event, || chat_log::Readiness {
            model_route: self.route().is_some(),
            persona: self.personas.pin().active().is_some(),
        });
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
    follow_up: Arc<dyn RejectionFollowUp>,
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

    pub fn rejection_follow_up(&self) -> Arc<dyn RejectionFollowUp> {
        Arc::clone(&self.follow_up)
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
    // Logs the starting setup; later flips log on the next read.
    driver.status();
    let refresh = driver.clone();
    let overrides = tokio::spawn(async move {
        while changes.changed().await.is_ok() {
            refresh.status();
            if let Ok(rows) = store.allowance_overrides().await {
                refresh.set_overrides(rows);
            }
        }
    });
    let feed = ChatFeed::new(Arc::new(driver.clone()), cache, staff(roster, access));
    let follow_up: Arc<dyn RejectionFollowUp> = Arc::new(driver.clone());
    let stop: Stop = Box::new(move || Box::pin(async move { driver.stop().await }));
    Ok((
        feed,
        ChatRuntime {
            stop: Some(stop),
            overrides,
            follow_up,
        },
    ))
}
