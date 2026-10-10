//! The Discord side of live serve: one gateway session for the configured
//! guild, the roster task, the reaction worker, the chat pilot
//! (`super::chat`), extraction (`super::extract`) and the delivery tick, all
//! sharing one `GuildCache`, one `LiveRoster`, one `CardDesk` and the API's
//! store, sessions and access policy.
//!
//! Shutdown order: gateway close (then its spawned interaction and
//! registration tasks) → chat stops (waiting questions refunded, running
//! ones finish within the grace or are cut, each concluded) → extraction
//! (feed, pipeline, `Rescans::close`; calls in flight cancelled) → roster and
//! reaction, card-refresh, header pre-generation and manual header rewrite
//! workers drain (a header rewrite in flight is abandoned) → the running tick finishes (polled throughout) →
//! (caller) HTTP drain → store close.

mod late;
mod ports;
mod stop_aware;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use twilight_model::id::Id;

use stop_aware::StopAware;

use super::{
    api::Composition,
    budget::ShutdownClock,
    chat::{self, ChatInputs, ChatRuntime, ServeAnswerer},
    chat_cards::ChatDesk,
    commands,
    extract::{self, Extraction, ExtractionStatus, Timing},
    health::GatewayProbe,
    tick::{self, TickLoop, TickStatus, card_kit, delivery_config, watch_list},
};
use crate::{
    api::{
        admin::config::SettingsChanged,
        auth::Clock,
        rescan::RescanDesk,
        state::{
            DeclineRetraction, DigestPost, DigestPostRequest, DigestPostResult, GuildAccess,
            HeaderRewritePort, ProposalCardRefresh,
        },
        write::ApiClock,
    },
    bot::{
        cards::{CardDesk, CardSettings, DeskDeps},
        delivery::{
            CardRefresh, Delivery, DigestOutcome, HeaderPregen, LogAlerts, ManualRequest,
            ManualRewrite, RefreshQueue, cards::CardKit,
        },
        events::{GuildScope, ReactionRouter, Router},
        gateway::{ConnectionStatus, EventSource, GatewayError, Live, RunExit, run_live},
        guild_cache::GuildCache,
        handler::{Fanout, MessageCounts, Reactions, press_port},
        identity,
        roster::{LiveRoster, RosterTask},
    },
    chat::{driver::DriverConfig, nudge::StoreRewriteSink},
    domain::{ids::RandomIds, schedule::SchedulePolicy, scheduler::SchedulerService},
    infrastructure::store::SqliteStore,
    runtime::{config::ServeConfig, error::Error, logging},
};

pub use late::{GatewayTransport, LateTransport};
use ports::{StaffAuthority, StoreIndex, StoreRoster};

/// How long the gateway waits for its close handshake on shutdown.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The gateway source and transport, the loop's clock and period, and
/// extraction's debounce and backlog pace.
pub struct Wiring<S, T> {
    pub source: S,
    pub transport: Arc<T>,
    pub clock: Clock,
    pub tick: Duration,
    pub extraction: Timing,
}

/// Shared state created before the API is composed (health reads it).
pub struct Prepared {
    pub cache: Arc<GuildCache>,
    pub probe: GatewayProbe,
    pub tick_status: Arc<TickStatus>,
    pub extraction: Arc<ExtractionStatus>,
    /// Bounds the stop phases and the tick's Discord calls once started.
    pub shutdown: ShutdownClock,
    live: Live,
}

pub fn prepare(config: &ServeConfig, tick: Duration) -> Prepared {
    let scope = scope(config);
    let cache = Arc::new(GuildCache::new(scope.guild_id));
    let router = Router::with_cache(scope, Arc::clone(&cache));
    let connection = ConnectionStatus::new();
    let probe = GatewayProbe {
        connection: connection.clone(),
        dropped: router.dropped(),
    };
    Prepared {
        cache,
        probe,
        tick_status: Arc::new(TickStatus::new(tick)),
        extraction: Arc::new(ExtractionStatus::default()),
        shutdown: ShutdownClock::default(),
        live: Live {
            router,
            status: connection,
        },
    }
}

fn scope(config: &ServeConfig) -> GuildScope {
    // Snowflakes are validated non-zero by the config parser.
    GuildScope {
        guild_id: Id::new(config.guild.guild_id),
        bossing_role_id: Id::new(config.guild.bossing_role_id),
    }
}

/// The running Discord side.
pub struct Discord {
    stop_gateway: Option<oneshot::Sender<()>>,
    gateway: Option<JoinHandle<RunExit>>,
    stop_workers: watch::Sender<bool>,
    workers: Vec<JoinHandle<()>>,
    extraction: Extraction,
    /// Polled by [`Discord::until`] rather than spawned: the tick's future
    /// is not provably `Send` (async-closure lease helper).
    tick: Option<Pin<Box<dyn Future<Output = ()>>>>,
    /// How the gateway task ended; `Err` if it panicked.
    exit: Option<Result<RunExit, ()>>,
    pub messages: MessageCounts,
    connection: ConnectionStatus,
    chat: Option<ChatRuntime>,
    /// Completed shutdown steps, in order.
    steps: Vec<&'static str>,
    /// Started when the shutdown signal fires; bounds every stop phase.
    pub shutdown: ShutdownClock,
}

/// One card desk (journalled posts, ✅/❌ answers) over the shared store,
/// rendering in the card kit's live style.
#[allow(clippy::too_many_arguments)]
fn card_desk<T: GatewayTransport>(
    config: &ServeConfig,
    store: &Arc<SqliteStore>,
    transport: &Arc<T>,
    clock: &Clock,
    roster: &Arc<LiveRoster>,
    access: &Arc<GuildAccess>,
    policy: &SchedulePolicy,
    decline_retraction: Option<DeclineRetraction>,
    cards: CardKit,
) -> ChatDesk<T> {
    CardDesk::new(
        DeskDeps {
            store: Arc::clone(store),
            transport: Arc::clone(transport),
            ids: RandomIds,
            clock: Arc::new(ApiClock(Arc::clone(clock))),
            directory: roster.clone(),
            authority: Arc::new(StaffAuthority {
                roster: Arc::clone(roster),
                access: Arc::clone(access),
            }),
            alerts: Arc::new(LogAlerts),
            decline_retraction,
        },
        CardSettings {
            zone: config.runtime.timezone,
            policy: policy.clone(),
            instance_id: config.instance_id.clone(),
        },
    )
    .with_cards(cards)
}

/// Recover the delivery journal, list the application's emojis for the
/// difficulty marks, then start everything. Nothing else calls Discord
/// until the gateway task first polls the source. Attaches the rescan
/// runner and the marks to the API state, so call it before HTTP serves.
pub async fn start<S, T>(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    composition: &mut Composition,
    prepared: Prepared,
    wiring: Wiring<S, T>,
) -> Result<Discord, Error>
where
    S: EventSource + 'static,
    T: GatewayTransport,
{
    // Before anything that can send: the reaction worker and commands post
    // as soon as the gateway is up, not only the tick.
    tick::recover(&store, (wiring.clock)()).await?;
    // Read once, before the card kit and the admin preview are composed.
    let marks = tick::difficulty_marks(&*wiring.transport).await;
    let Prepared {
        cache,
        tick_status,
        extraction: extraction_status,
        shutdown,
        live,
        probe,
    } = prepared;
    // Every Discord call made from here on (tick, card desk, manual digest,
    // card refresh, decline retraction, commands, chat, extraction, roster,
    // identity) ends by the shutdown deadline with the outcome the
    // transport's own deadline would give, so a journalled send in flight at
    // the cutoff is finalised, not dropped. A pass-through until shutdown.
    let transport = Arc::new(StopAware::new(
        Arc::clone(&wiring.transport),
        shutdown.clone(),
    ));
    let connection = live.status.clone();
    let scope = scope(config);
    let access = Arc::clone(&composition.access);
    let auth = Arc::clone(&composition.admin.auth);
    let policy = composition.admin.state.policy.clone();
    cache.set_watch(watch_list(&composition.settings));
    let roster = Arc::new(LiveRoster::new(Arc::clone(&cache)));
    let (stop_workers, stopped) = watch::channel(false);

    let (roster_jobs, roster_queue) = mpsc::unbounded_channel();
    let roster_task = RosterTask::new(
        StoreRoster {
            store: Arc::clone(&store),
            auth,
            member: composition.admin.member.clone(),
            access: Arc::clone(&access),
            avatars: composition.admin.state.avatars.clone(),
        },
        Arc::clone(&transport),
        Arc::clone(&cache),
        scope,
        Arc::clone(&roster),
        connection.clone(),
        stopped.clone(),
    );

    let decline_retraction: DeclineRetraction = {
        let store = Arc::clone(&store);
        let transport = Arc::clone(&transport);
        let roster = Arc::clone(&roster);
        let cache = Arc::clone(&cache);
        let config = delivery_config(&config.instance_id, policy.clone(), &composition.settings);
        Arc::new(move |run_id, user_id, now| {
            let store = Arc::clone(&store);
            let transport = Arc::clone(&transport);
            let roster = Arc::clone(&roster);
            let cache = Arc::clone(&cache);
            let config = config.clone();
            Box::pin(async move {
                let alerts = LogAlerts;
                let mut delivery = Delivery::new(
                    &*store,
                    RandomIds,
                    &*transport,
                    &alerts,
                    &*roster,
                    &*cache,
                    config,
                );
                let _ = delivery
                    .retract_decline_notice(&run_id, &user_id, now)
                    .await;
            })
        })
    };
    // Every card user (tick, refresh, digest post, debug, header
    // pre-generation, the only one that calls the model, and the proposal
    // card desk) shares the kit, so no heading rewrite outlasts the
    // shutdown deadline, and all of them share one record of which posts
    // are Components V2.
    let mut kit = card_kit(
        config.runtime.http.boss_dir.as_deref(),
        Arc::clone(&composition.admin.state.catalog),
        marks.clone(),
        composition.models.as_ref(),
        Arc::clone(&composition.personas),
        settings_changes(composition),
        Arc::new(StoreRewriteSink::new(
            Arc::clone(&store),
            Arc::clone(&wiring.clock),
        )),
    );
    kit.v2.avatar = Some({
        let cache = Arc::clone(&cache);
        Arc::new(move || cache.self_avatar_url())
    });
    // Members open the public portal; the admin host is tailnet-only, so
    // without a public listener the digest has no portal button, notices a
    // plain "via portal" mark and cards no links (nor while the live
    // `self_service.public_portal` switch is off, see `card_kit` and
    // extraction's live self-service settings).
    let portal = config
        .runtime
        .public_bind
        .and(config.runtime.http.public_host.as_deref())
        .map(|host| format!("https://{host}"));
    kit.v2.portal = portal.clone();
    let cards = stop_aware::cards(kit, shutdown.clone());
    // One desk for extraction cards, chat cards and the reaction worker's
    // ✅/❌, which all read the same stored cards.
    let desk = Arc::new(
        card_desk(
            config,
            &store,
            &transport,
            &wiring.clock,
            &roster,
            &access,
            &policy,
            Some(Arc::clone(&decline_retraction)),
            cards.clone(),
        )
        .with_run_ends(composition.admin.state.run_ends_source()),
    );
    let proposal_refresh: ProposalCardRefresh = {
        let desk = Arc::clone(&desk);
        Arc::new(move |proposal_ids| {
            let desk = Arc::clone(&desk);
            Box::pin(async move { desk.refresh_proposals(&proposal_ids).await })
        })
    };
    match Arc::get_mut(&mut composition.admin.state) {
        Some(state) => {
            state.proposal_refresh = Some(proposal_refresh);
            state.decline_retraction = Some(decline_retraction);
            state.marks = marks;
        }
        None => {
            return Err(Error::Startup(
                "the proposal-card refresh could not be attached".into(),
            ));
        }
    }
    let rsvp = ReactionRouter::new(
        StoreIndex(Arc::clone(&store)),
        SchedulerService::new(
            Arc::clone(&store),
            RandomIds,
            ApiClock(Arc::clone(&wiring.clock)),
        )
        .with_attendance(policy.attendance)
        .with_run_ends(composition.admin.state.run_ends_source()),
    );
    let rsvp_replay = crate::bot::rsvp_replay::RsvpReplay::new(
        Arc::clone(&store),
        Arc::clone(&transport),
        Arc::clone(&wiring.clock),
        Arc::new(LogAlerts),
    );
    let (reaction_jobs, reaction_queue) = mpsc::unbounded_channel();
    // V2 proposal-card presses go to the same sequential worker.
    let (press_jobs, press_queue) = mpsc::unbounded_channel();

    let (guild_ready, ready) = watch::channel(false);
    let state = &composition.admin.state;
    let mut extraction = extract::start(extract::Inputs {
        store: Arc::clone(&store),
        models: composition.models.clone(),
        settings: composition.settings.clone(),
        changes: state.config.as_ref().map(|desk| desk.subscribe()),
        desk: Arc::clone(&desk),
        transport: Arc::clone(&transport),
        cache: Arc::clone(&cache),
        roster: Arc::clone(&roster),
        bosses: Arc::clone(&state.catalog),
        policy: policy.clone(),
        zone: config.runtime.timezone,
        clock: Arc::clone(&wiring.clock),
        status: extraction_status,
        timing: wiring.extraction,
        guild_ready: ready.clone(),
        portal,
        run_ends: state.run_ends_source(),
    });
    // Nothing has cloned the state yet: compose returned the only handle.
    match Arc::get_mut(&mut composition.admin.state) {
        Some(state) => {
            state.rescans = extraction
                .rescans
                .clone()
                .map(|runner| Arc::new(RescanDesk::new(runner)));
        }
        None => {
            extraction.stop(&ShutdownClock::default()).await;
            return Err(Error::Startup(
                "the rescan runner could not be attached".into(),
            ));
        }
    }
    // The kit composed above, shared with every card user below.
    let quiet = Arc::new(AtomicBool::new(
        composition.settings.notifications.quiet_mode,
    ));
    let post_channel = Arc::new(RwLock::new(composition.settings.posting.channel_id.clone()));
    let digest_post: DigestPost = {
        let store = Arc::clone(&store);
        let transport = Arc::clone(&transport);
        let roster = Arc::clone(&roster);
        let cache = Arc::clone(&cache);
        let cards = cards.clone();
        let post_channel = Arc::clone(&post_channel);
        let delivery = delivery_config(&config.instance_id, policy.clone(), &composition.settings);
        let connection = connection.clone();
        Arc::new(move |request: DigestPostRequest| {
            let store = Arc::clone(&store);
            let transport = Arc::clone(&transport);
            let roster = Arc::clone(&roster);
            let cache = Arc::clone(&cache);
            let cards = cards.clone();
            let post_channel = Arc::clone(&post_channel);
            let mut delivery_config = delivery.clone();
            let connection = connection.clone();
            Box::pin(async move {
                delivery_config.post_channel_id = post_channel
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let alerts = LogAlerts;
                let mut delivery = Delivery::new(
                    &*store,
                    RandomIds,
                    &*transport,
                    &alerts,
                    &*roster,
                    &*cache,
                    delivery_config,
                )
                .with_cards(cards)
                .with_admission_gate(move || connection.delivery_eligibility());
                match delivery
                    .post_requested_digest(
                        request.at,
                        request.week_start,
                        request.channel_id.as_deref(),
                    )
                    .await
                {
                    Ok(report) if report.outcome == DigestOutcome::ClockRolledBack => {
                        DigestPostResult::NewerWeekAlreadyPosted
                    }
                    Ok(report) if report.outcome == DigestOutcome::Attempted => {
                        DigestPostResult::Completed
                    }
                    Ok(_) | Err(_) => DigestPostResult::Unavailable,
                }
            })
        })
    };
    match Arc::get_mut(&mut composition.admin.state) {
        Some(state) => state.digest_post = Some(digest_post),
        None => {
            extraction.stop(&ShutdownClock::default()).await;
            return Err(Error::Startup(
                "the manual digest post could not be attached".into(),
            ));
        }
    }
    let debug = commands::DebugParts {
        cards: cards.clone(),
        roster: Arc::clone(&roster),
        quiet: Arc::clone(&quiet),
        post_channel: Arc::clone(&post_channel),
        test_channel: config.guild.test_channel_id.map(|id| id.to_string()),
        instance_id: config.instance_id.clone(),
    };
    let refresh = Arc::new(CardRefresh {
        store: Arc::clone(&store),
        transport: Arc::clone(&transport),
        members: roster.clone(),
        cards: cards.clone(),
        policy: policy.clone(),
        quiet: Arc::clone(&quiet),
        now: Arc::clone(&wiring.clock),
    });
    // `/debug rewrite` and the admin portal's manual header rewrite: one
    // worker, editing through the refresh path.
    let manual = Arc::new(ManualRewrite::new(Arc::clone(&refresh)));
    let header_rewrite: HeaderRewritePort = {
        let manual = Arc::clone(&manual);
        Arc::new(move |request: ManualRequest| {
            let manual = Arc::clone(&manual);
            Box::pin(async move { manual.start(request).await })
        })
    };
    match Arc::get_mut(&mut composition.admin.state) {
        Some(state) => state.header_rewrite = Some(header_rewrite),
        None => {
            extraction.stop(&ShutdownClock::default()).await;
            return Err(Error::Startup(
                "the manual header rewrite could not be attached".into(),
            ));
        }
    }
    // Every committed run write (reactions, commands, API, chat, proposals,
    // the tick) queues a card refresh; one task drains it.
    let refresh_queue = Arc::new(RefreshQueue::default());
    let queued = Arc::clone(&refresh_queue);
    store.observe_run_writes(Arc::new(move |runs: &[String]| queued.request(runs)));
    // Persona headers are rewritten ahead of their sends (a daily batch and
    // catch-up), never on them.
    // The daily batch time is read live, like the message style.
    let header_time = settings_changes(composition);
    let pregen = HeaderPregen::new(
        Arc::clone(&store),
        roster.clone(),
        cards.clone(),
        policy.clone(),
        Arc::clone(&wiring.clock),
    )
    .with_time(Arc::new(move || {
        header_time
            .borrow()
            .settings
            .notifications
            .header_generation_time
    }));
    let tick = TickLoop {
        store: Arc::clone(&store),
        transport: Arc::clone(&transport),
        cache: Arc::clone(&cache),
        roster: Arc::clone(&roster),
        clock: Arc::clone(&wiring.clock),
        period: wiring.tick,
        seeds: config.seeds.clone(),
        config: delivery_config(&config.instance_id, policy, &composition.settings),
        status: tick_status,
        claim_gate: {
            let connection = connection.clone();
            Arc::new(move || connection.delivery_eligibility())
        },
        cards,
        quiet,
        post_channel,
    };

    let dispatcher = match commands::factory(
        Arc::clone(&composition.admin.state),
        Arc::clone(&store),
        Arc::clone(&cache),
        Arc::clone(&transport),
        debug,
        press_port(press_jobs),
    ) {
        Ok(dispatcher) => dispatcher,
        Err(error) => {
            extraction.stop(&ShutdownClock::default()).await;
            return Err(error);
        }
    };
    let ready_transport = Arc::clone(&transport);
    let owner_access = Arc::clone(&access);
    let chat_roster = Arc::clone(&roster);
    let mut handler = Fanout::new(
        scope.guild_id,
        Arc::clone(&transport),
        dispatcher,
        Arc::new(move || owner_access.owner()),
        roster,
        roster_jobs,
        reaction_jobs,
        connection.clone(),
        Box::new(move |application| ready_transport.application_ready(application)),
        guild_ready,
        Arc::clone(&wiring.clock),
    )
    .with_feed(extraction.feed.take());
    let messages = handler.messages.clone();
    let started = chat::start(ChatInputs {
        config: DriverConfig::default(),
        answerer: ServeAnswerer {
            store: Arc::clone(&store),
            models: composition.models.clone(),
            personas: Arc::clone(&composition.personas),
            config: Arc::clone(
                composition
                    .admin
                    .state
                    .config
                    .as_ref()
                    .expect("serve always composes config settings"),
            ),
            settings: settings_changes(composition),
            cache: Arc::clone(&cache),
            catalog: Arc::clone(&composition.admin.state.catalog),
            guides: composition.knowledge.clone(),
            policy: composition.admin.state.policy.clone(),
            guild_id: config.guild.guild_id.to_string(),
            pilot_role: access.pilot_role.clone(),
            clock: Arc::clone(&wiring.clock),
            desk: Arc::clone(&desk),
        },
        transport: Arc::clone(&transport),
        handle: composition.admin.state.chat.clone(),
        roster: chat_roster,
        access: Arc::clone(&access),
    })
    .await;
    let (feed, chat) = match started {
        Ok(started) => started,
        Err(error) => {
            extraction.stop(&ShutdownClock::default()).await;
            return Err(error);
        }
    };
    let follow_up = chat.rejection_follow_up();
    handler.chat = Some(feed);

    let mut workers = vec![
        tokio::spawn(roster_task.run(roster_queue)),
        tokio::spawn(
            Reactions {
                desk,
                rsvp,
                rsvp_replay: Some(rsvp_replay),
                follow_up: Some(follow_up),
                decline_retraction: composition.admin.state.decline_retraction.clone(),
                clock: Arc::clone(&wiring.clock),
            }
            .run_with_replay(
                reaction_queue,
                press_queue,
                connection.clone(),
                stopped.clone(),
            ),
        ),
    ];
    let refresh_stop = stopped.clone();
    workers.push(tokio::spawn(async move {
        refresh.run(&refresh_queue, refresh_stop).await;
    }));
    let pregen_stop = stopped.clone();
    workers.push(tokio::spawn(async move { pregen.run(pregen_stop).await }));
    let manual_stop = stopped.clone();
    workers.push(tokio::spawn(async move { manual.run(manual_stop).await }));
    let identity_dir = config.runtime.http.identity_dir.as_deref();
    workers.extend(identity::spawn(
        identity_dir,
        Arc::clone(&transport),
        Arc::clone(&cache),
        stopped.clone(),
    ));
    let tick: Pin<Box<dyn Future<Output = ()>>> = Box::pin(tick.run(ready, stopped));
    let (stop_gateway, stop_requested) = oneshot::channel::<()>();
    let mut source = wiring.source;
    let gateway = tokio::spawn(async move {
        let exit = run_live(
            &mut source,
            &mut handler,
            live,
            DRAIN_TIMEOUT,
            async {
                let _ = stop_requested.await;
            },
            gateway_error,
        )
        .await;
        handler.finish().await;
        exit
    });
    Ok(Discord {
        stop_gateway: Some(stop_gateway),
        gateway: Some(gateway),
        stop_workers,
        workers,
        extraction,
        tick: Some(tick),
        exit: None,
        messages,
        connection: probe.connection,
        chat: Some(chat),
        steps: Vec::new(),
        shutdown,
    })
}

/// Live settings for chat: the config desk's changes, else the startup values.
fn settings_changes(composition: &Composition) -> watch::Receiver<SettingsChanged> {
    match &composition.admin.state.config {
        Some(desk) => desk.subscribe(),
        None => {
            watch::channel(SettingsChanged {
                revision: 0,
                section: None,
                actor: None,
                settings: Arc::new(composition.settings.clone()),
            })
            .1
        }
    }
}

fn gateway_error(error: GatewayError) {
    logging::event(
        "WARN",
        "gateway_receive_failed",
        json!({"error": format!("{error:?}")}),
    );
}

impl Discord {
    /// Run until `shutdown` or until the gateway ends on its own (a fatal
    /// close), then stop everything in order.
    pub async fn until(&mut self, shutdown: impl Future<Output = ()>) {
        enum Ended {
            Shutdown,
            Gateway(Result<RunExit, ()>),
            Tick,
        }
        tokio::pin!(shutdown);
        while let Some(gateway) = self.gateway.as_mut() {
            let tick = async {
                match self.tick.as_mut() {
                    Some(tick) => tick.await,
                    None => std::future::pending().await,
                }
            };
            let ended = tokio::select! {
                () = &mut shutdown => Ended::Shutdown,
                exit = gateway => Ended::Gateway(exit.map_err(drop)),
                () = tick => Ended::Tick,
            };
            match ended {
                Ended::Shutdown => {
                    self.shutdown.start();
                    break;
                }
                Ended::Gateway(exit) => {
                    self.gateway = None;
                    let fatal = matches!(exit, Ok(RunExit::Closed { .. }));
                    self.exit = Some(exit);
                    if fatal {
                        self.closed_for_good().await;
                        shutdown.await;
                        self.shutdown.start();
                        return;
                    }
                }
                // Only when its guild-ready sender went away with the gateway.
                Ended::Tick => self.tick = None,
            }
        }
        self.stop().await;
    }

    /// Idempotent ordered stop; see the module docs.
    pub async fn stop(&mut self) {
        // The tick is polled from the start: a tick suspended inside a store
        // write would otherwise block every write below (the gateway's
        // spawned tasks, chat, extraction, workers).
        let tick = self.tick.take();
        let steps = &mut self.steps;
        let (stop_gateway, gateway, exit) = (
            self.stop_gateway.take(),
            self.gateway.take(),
            &mut self.exit,
        );
        let chat = self.chat.take();
        let extraction = &mut self.extraction;
        let stop_workers = &self.stop_workers;
        let workers = &mut self.workers;
        let clock = &self.shutdown;
        let rest = async move {
            if let Some(stop) = stop_gateway {
                let _ = stop.send(());
            }
            if let Some(mut gateway) = gateway {
                // The run's own close and task-drain graces bound it before
                // the deadline does.
                match clock.bounded("gateway", Duration::MAX, &mut gateway).await {
                    Some(joined) => *exit = Some(joined.map_err(drop)),
                    // Cut by the budget: no `RunExit` (its dropped-event
                    // count is unknown) and not a failure; only a panic
                    // fails serve.
                    None => {
                        gateway.abort();
                        if let Err(error) = gateway.await
                            && error.is_panic()
                        {
                            *exit = Some(Err(()));
                        } else {
                            logging::event("WARN", "gateway_aborted", json!({}));
                        }
                    }
                }
            }
            if !steps.contains(&"gateway_closed") {
                steps.push("gateway_closed");
                logging::event(
                    "INFO",
                    "gateway_closed",
                    json!({"exit": format!("{:?}", exit)}),
                );
            }
            // No message can reach chat or extraction any more; their cards
            // and replies go out before the workers stop.
            if let Some(mut chat) = chat {
                // Its waits end by the budget, but the cut, abort and join
                // always run, so nothing is dropped half-stopped; the log
                // writes of aborted questions may take up to 1 s past it
                // (`chat::driver::LOG_BUDGET`, out of the store reserve).
                let end = clock.phase_end();
                chat.stop(end).await;
                if end.is_some_and(|end| tokio::time::Instant::now() >= end) {
                    clock.cut("chat");
                }
                logging::event("INFO", "chat_stopped", json!({}));
                steps.push("chat_stopped");
            }
            extraction.stop(clock).await;
            if !steps.contains(&"extraction_stopped") {
                steps.push("extraction_stopped");
            }
            // Every queue sender is gone: workers drain, the tick stops.
            stop_workers.send_replace(true);
            for mut worker in workers.drain(..) {
                if clock
                    .bounded("workers", Duration::MAX, &mut worker)
                    .await
                    .is_none()
                {
                    worker.abort();
                    let _ = worker.await;
                }
            }
            if !steps.contains(&"workers_stopped") {
                steps.push("workers_stopped");
            }
        };
        // The tick is awaited, never cut. It ends near the deadline anyway:
        // its Discord calls end by it (`stop_aware`; it never calls the model),
        // no send is admitted once the gateway has closed, its reads fail
        // at once past the cutoff (`ShutdownClock::refusing_reads`), and
        // what is left are atomic writes, each holding the one writer
        // connection only for its own SQL (SQLite `busy_timeout` 5 s).
        match tick {
            Some(tick) => {
                tokio::join!(rest, tick);
                self.steps.push("tick_stopped");
            }
            None => rest.await,
        }
    }

    /// Parent decision: a fatal close (4004 token, 4014 intents) must not
    /// exit, since a restart policy would re-IDENTIFY with the shared
    /// production token every minute. Log once, stop the Discord side and
    /// keep serving HTTP with health `discord: closed` until shutdown.
    async fn closed_for_good(&mut self) {
        self.connection.closed();
        if let Some(Ok(RunExit::Closed { reason })) = self.exit {
            logging::event(
                "ERROR",
                "gateway_closed_for_good",
                json!({"code": reason.code(), "message": reason.to_string()}),
            );
        }
        self.stop().await;
    }

    pub fn steps(&self) -> &[&'static str] {
        &self.steps
    }

    /// A panicked gateway task fails serve; a fatal close does not (see
    /// [`Self::closed_for_good`]).
    pub fn result(&self) -> Result<(), Error> {
        match self.exit {
            Some(Err(())) => Err(Error::Startup("the Discord gateway task failed".into())),
            _ => Ok(()),
        }
    }
}
