//! The delivery tick loop: recovery once, then one tick every
//! `KANADE_TICK_SECONDS` under its own lease. The outbox drain, reminders,
//! digests and expiry all run inside `Delivery::tick_at` in v4's order.
//! A tick is never cancelled midway: stopping waits for the running one.
//! Cards read the boss catalog and art and rewrite the day-of heading
//! through the `rewrite` model role ([`card_kit`]).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use super::settings;
use crate::{
    api::{admin::config::SettingsChanged, auth::Clock},
    bot::{
        delivery::{
            DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, DeliveryError, LogAlerts,
            TickReport,
            cards::{ArtSource, CardKit, HeadingRewrite, PersonaSource},
        },
        gateway::DeliveryEligibility,
        guild_cache::{GuildCache, WatchList},
        ids::parse_id,
        roster::LiveRoster,
        transport::DiscordTransport,
    },
    chat::{
        nudge::{GovernedRewriter, RewriteReserve, SharedRewriter},
        persona::{CompiledPersona, PersonaStore},
    },
    domain::notify::DeliveryJournal,
    domain::{
        catalog::BossTable, ids::RandomIds, members::MemberStore, notify::DEFAULT_MAX_NOTICE_AGE,
        settings::RuntimeSettings,
    },
    infrastructure::{
        files::BossArt,
        llm::{
            LlmProvider,
            governor::{ModelClient, Role},
            setup::ModelStack,
        },
        store::SqliteStore,
    },
    runtime::{config::SettingSeeds, error::Error, logging},
};
use chrono::{DateTime, Utc};

const STARTING: u8 = 0;
const RUNNING: u8 = 1;
const STOPPED: u8 = 2;

/// The tick's state for health.
#[derive(Debug)]
pub struct TickStatus {
    period: Duration,
    state: AtomicU8,
    last: Mutex<Option<Instant>>,
}

impl TickStatus {
    pub fn new(period: Duration) -> Self {
        Self {
            period,
            state: AtomicU8::new(STARTING),
            last: Mutex::new(None),
        }
    }

    /// `starting` (waiting for the guild or recovery), `running`, `stalled`
    /// (no completed tick for three periods plus five minutes, which covers
    /// a slow tick of capped sends) or `stopped`.
    pub fn state(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            STOPPED => "stopped",
            STARTING => "starting",
            _ => match self.last_age() {
                Some(age) if age > self.period * 3 + Duration::from_secs(300) => "stalled",
                Some(_) => "running",
                None => "starting",
            },
        }
    }

    fn last_age(&self) -> Option<Duration> {
        self.last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|at| at.elapsed())
    }

    pub fn last_age_seconds(&self) -> Option<u64> {
        self.last_age().map(|age| age.as_secs())
    }

    fn ticked(&self) {
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    fn set(&self, state: u8) {
        self.state.store(state, Ordering::Relaxed);
    }
}

/// Everything the tick loop owns.
pub struct TickLoop<T> {
    pub store: Arc<SqliteStore>,
    pub transport: Arc<T>,
    pub cache: Arc<GuildCache>,
    pub roster: Arc<LiveRoster>,
    pub clock: Clock,
    pub period: Duration,
    pub seeds: SettingSeeds,
    pub config: DeliveryConfig,
    pub status: Arc<TickStatus>,
    pub(super) claim_gate: Arc<dyn Fn() -> Option<DeliveryEligibility> + Send + Sync>,
    pub cards: CardKit,
    /// Shared with the reaction worker's card edits.
    pub quiet: Arc<AtomicBool>,
    /// The live post channel, shared with `/debug ping`.
    pub post_channel: Arc<RwLock<Option<String>>>,
}

/// The heading's rewriter over the governed `rewrite` role, each request
/// sized by the live resolved rewrite reserve.
pub fn heading_rewriter<P: LlmProvider + 'static>(
    client: Arc<ModelClient<P>>,
    reserve: RewriteReserve,
) -> SharedRewriter {
    SharedRewriter(Arc::new(
        GovernedRewriter::new(client).with_reserve(reserve),
    ))
}

/// Card inputs for the tick and card edits: the catalog, the boss art
/// directory (none: no pictures) and the day-of heading rewrite (the
/// `rewrite` role through the nudge rewriter, the guild's default persona;
/// no role or no persona: v4's heading).
pub fn card_kit(
    boss_dir: Option<&Path>,
    catalog: Arc<BossTable>,
    models: Option<&Arc<ModelStack>>,
    personas: Arc<PersonaStore>,
    settings: watch::Receiver<SettingsChanged>,
) -> CardKit {
    let rewriter = models
        .filter(|stack| stack.has_role(Role::Rewrite))
        .map(|stack| {
            let resolve = super::context::resolver(Arc::clone(stack), settings, Role::Rewrite);
            heading_rewriter(
                Arc::clone(&stack.client),
                Arc::new(move |alias: &str| resolve(alias).reserve),
            )
        });
    let persona: PersonaSource = Arc::new(move || {
        let snapshot = personas.pin();
        let active = snapshot.active()?;
        Some(CompiledPersona::compile(&active.bundle.value, None))
    });
    let art = boss_dir.map(|dir| Arc::new(BossArt::new(dir)) as Arc<dyn ArtSource>);
    logging::event(
        "INFO",
        "cards_ready",
        json!({"art": art.is_some(), "heading_rewrite": rewriter.is_some()}),
    );
    CardKit {
        catalog: Some(catalog),
        art,
        heading: HeadingRewrite {
            rewriter,
            persona: Some(persona),
        },
    }
}

/// The delivery settings from startup (policy, which the API also fixes at
/// startup) and the post channel and quiet mode from `settings`.
pub fn delivery_config(
    instance_id: &str,
    policy: crate::domain::schedule::SchedulePolicy,
    settings: &RuntimeSettings,
) -> DeliveryConfig {
    DeliveryConfig {
        instance_id: instance_id.to_owned(),
        policy,
        post_channel_id: settings.posting.channel_id.clone(),
        quiet_mode: settings.notifications.quiet_mode,
        max_sends_per_tick: DEFAULT_MAX_SENDS_PER_TICK,
        max_notice_age: DEFAULT_MAX_NOTICE_AGE,
    }
}

/// The extractor's watch list from settings; unparsable ids are skipped.
pub fn watch_list(settings: &RuntimeSettings) -> WatchList {
    let ids = |list: &[String]| list.iter().filter_map(|id| parse_id(id)).collect();
    WatchList {
        channel_ids: ids(&settings.watching.channel_ids),
        category_ids: ids(&settings.watching.category_ids),
    }
}

impl<T: DiscordTransport> TickLoop<T> {
    /// Wait for the guild and its initial roster reconciliation attempt, then
    /// tick until `stop`. Returns after the running tick completes.
    /// [`recover`] must already have run.
    pub async fn run(
        self,
        mut guild_ready: watch::Receiver<bool>,
        mut stop: watch::Receiver<bool>,
    ) {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stop| *stop) => return self.status.set(STOPPED),
            ready = guild_ready.wait_for(|ready| *ready) => if ready.is_err() {
                return self.status.set(STOPPED);
            },
        }
        tokio::select! {
            biased;
            _ = stop.wait_for(|stop| *stop) => return self.status.set(STOPPED),
            _ = self.roster.reconciled() => {}
        }
        let alerts = LogAlerts;
        let mut delivery = Delivery::new(
            &*self.store,
            RandomIds,
            &*self.transport,
            &alerts,
            &*self.roster,
            &*self.cache,
            self.config.clone(),
        )
        .with_cards(self.cards.clone())
        .with_admission_gate({
            let gate = Arc::clone(&self.claim_gate);
            move || gate()
        });
        let mut interval = tokio::time::interval(self.period);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        self.status.set(RUNNING);
        loop {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => break,
                _ = interval.tick() => {}
            }
            self.refresh(&mut delivery.config).await;
            let now = (self.clock)();
            if let Err(error) = delivery.drain_decline_notices(now).await {
                tick_failed("decline", &error);
            }
            match delivery.tick_at(now).await {
                Ok(report) => {
                    self.status.ticked();
                    log_report(&report);
                }
                Err(error) => tick_failed("tick", &error),
            }
        }
        self.status.set(STOPPED);
        logging::event("INFO", "tick_stopped", json!({}));
    }

    /// Settings and members edited through the portal take effect on the
    /// next tick; a failed read keeps the previous values.
    async fn refresh(&self, config: &mut DeliveryConfig) {
        if let Ok(settings) = settings::load(&self.store, &self.seeds).await {
            config.post_channel_id = settings.posting.channel_id.clone();
            config.quiet_mode = settings.notifications.quiet_mode;
            self.quiet.store(config.quiet_mode, Ordering::Relaxed);
            config.post_channel_id.clone_into(
                &mut self
                    .post_channel
                    .write()
                    .unwrap_or_else(PoisonError::into_inner),
            );
            self.cache.set_watch(watch_list(&settings));
        }
        if let Ok(rows) = self.store.list_members().await {
            self.roster.replace(rows);
        }
    }
}

/// Once per process, before anything can send (tick, cards, commands):
/// attempts a previous process left in flight become indeterminate and are
/// never resent. Running it later would also catch this process's own
/// in-flight sends.
pub async fn recover(store: &SqliteStore, now: DateTime<Utc>) -> Result<(), Error> {
    match store.recover_on_start(now).await {
        Ok(recovery) => {
            logging::event(
                "INFO",
                "delivery_recovered",
                json!({
                    "indeterminate": recovery.indeterminate.len(),
                    "orphaned_leases": recovery.orphaned_leases,
                }),
            );
            Ok(())
        }
        // Journal text can carry store paths; the kind is enough here.
        Err(_) => Err(Error::Startup(
            "delivery journal recovery failed; the store must be checked before serving".into(),
        )),
    }
}

fn tick_failed(step: &'static str, error: &DeliveryError) {
    // Journal/scheduler text can carry store detail; the variant is enough.
    let kind = match error {
        DeliveryError::Journal(_) => "journal",
        DeliveryError::Scheduler(_) => "scheduler",
        DeliveryError::Date(_) => "date",
    };
    logging::event("WARN", "tick_failed", json!({"step": step, "kind": kind}));
}

fn log_report(report: &TickReport) {
    let sends = report.dispatch.sends.len() + report.notices.sends.len();
    if sends == 0 && report.materialised.is_empty() && report.done.is_empty() {
        return;
    }
    logging::event(
        "INFO",
        "tick",
        json!({
            "materialised": report.materialised.len(),
            "done": report.done.len(),
            "sends": sends,
            "deferred": report.dispatch.deferred,
        }),
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::chat::nudge::{NudgeRewriter, RewritePrompt};
    use crate::chat::persona::{NudgeMood, PersonaId, parse_bundle};
    use crate::infrastructure::llm::governor::{
        Governor, GovernorConfig, GovernorPolicy, GroupConfig, RoleConfig, XorShift,
    };
    use crate::infrastructure::llm::{
        CompletionResponse, ExecutionLimits, FakeAction, FakeProvider, FinishReason, RetryPolicy,
    };

    const ALIAS: &str = "rewriter";

    fn client(reply: &str) -> (Arc<FakeProvider>, Arc<ModelClient<FakeProvider>>) {
        let config = GovernorConfig {
            groups: vec![GroupConfig {
                name: "local".into(),
                backend: "local".into(),
                permits: 1,
                requests_per_min: 6_000,
                burst: Some(1_000),
                aliases: vec![ALIAS.into()],
            }],
            roles: [(
                Role::Rewrite,
                RoleConfig {
                    alias: ALIAS.into(),
                    external: true,
                },
            )]
            .into_iter()
            .collect::<BTreeMap<_, _>>(),
            policy: GovernorPolicy::default(),
        };
        let governor = Arc::new(Governor::new(&config, Arc::new(XorShift::new(1))).unwrap());
        let provider = Arc::new(FakeProvider::new([FakeAction::Response(
            CompletionResponse {
                reasoning_content: None,
                reasoning_tokens: None,
                model: ALIAS.into(),
                content: Some(reply.into()),
                tool_calls: Vec::new(),
                finish_reason: FinishReason::Stop,
                usage: None,
            },
        )]));
        let client = ModelClient::new(
            governor,
            Arc::clone(&provider),
            ExecutionLimits::default(),
            RetryPolicy::default(),
        )
        .unwrap();
        (provider, Arc::new(client))
    }

    fn persona() -> CompiledPersona {
        let text = include_str!("../../../config/personas/bundles/kanade.yaml");
        let bundle = parse_bundle(text, &PersonaId::parse("kanade").unwrap()).unwrap();
        CompiledPersona::compile(&bundle, None)
    }

    #[tokio::test]
    async fn external_rewrite_sends_raw_names_ids_and_urls_via_the_governor() {
        let (provider, client) = client("Bossing day — {day}!");
        let rewriter = GovernedRewriter::new(client);
        let prompt = RewritePrompt::build(
            &persona(),
            NudgeMood::Playful,
            "SyntheticMember 999000111222333444 https://synthetic.invalid/roster?q=1 {day}",
        );
        assert_eq!(
            rewriter
                .rewrite(&prompt, Duration::from_secs(2))
                .await
                .unwrap(),
            "Bossing day — {day}!"
        );
        let requests = provider.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].model, ALIAS);
        assert_eq!(requests[0].messages, prompt.messages());
        let sent = serde_json::to_string(&requests[0].messages).unwrap();
        for raw in [
            "SyntheticMember",
            "999000111222333444",
            "https://synthetic.invalid/roster?q=1",
        ] {
            assert!(sent.contains(raw), "{raw} missing from {sent}");
        }
    }
}
