//! The Discord side of live serve: one gateway session for the configured
//! guild, the roster task, the reaction worker and the delivery tick, all
//! sharing one `GuildCache`, one `LiveRoster` and the API's store, sessions
//! and access policy. Chat and extraction are not wired (messages are only
//! counted).
//!
//! Shutdown order: gateway close (then its spawned interaction and
//! registration tasks) → roster and reaction workers drain → the running
//! tick finishes → (caller) HTTP drain → store close.

mod late;
mod ports;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use twilight_model::id::Id;

use super::{
    api::Composition,
    commands,
    health::GatewayProbe,
    tick::{self, TickLoop, TickStatus, delivery_config, watch_list},
};
use crate::{
    api::{auth::Clock, write::ApiClock},
    bot::{
        cards::{CardDesk, CardSettings, DeskDeps},
        delivery::LogAlerts,
        events::{GuildScope, ReactionRouter, Router},
        gateway::{ConnectionStatus, EventSource, GatewayError, Live, RunExit, run_live},
        guild_cache::GuildCache,
        handler::{Fanout, MessageCounts, Reactions},
        roster::{LiveRoster, RosterTask},
    },
    domain::{ids::RandomIds, scheduler::SchedulerService},
    infrastructure::store::SqliteStore,
    runtime::{config::ServeConfig, error::Error, logging},
};

pub use late::{GatewayTransport, LateTransport};
use ports::{StaffAuthority, StoreIndex, StoreRoster};

/// How long the gateway waits for its close handshake on shutdown.
pub const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// The gateway source and transport, and the loop's clock and period.
pub struct Wiring<S, T> {
    pub source: S,
    pub transport: Arc<T>,
    pub clock: Clock,
    pub tick: Duration,
}

/// Shared state created before the API is composed (health reads it).
pub struct Prepared {
    pub cache: Arc<GuildCache>,
    pub probe: GatewayProbe,
    pub tick_status: Arc<TickStatus>,
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
    /// Polled by [`Discord::until`] rather than spawned: the tick's future
    /// is not provably `Send` (async-closure lease helper).
    tick: Option<Pin<Box<dyn Future<Output = ()>>>>,
    /// How the gateway task ended; `Err` if it panicked.
    exit: Option<Result<RunExit, ()>>,
    pub messages: MessageCounts,
    connection: ConnectionStatus,
    /// Completed shutdown steps, in order.
    steps: Vec<&'static str>,
}

/// Recover the delivery journal, then start everything. Nothing connects
/// until the gateway task first polls the source.
pub async fn start<S, T>(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    composition: &Composition,
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
    let Prepared {
        cache,
        tick_status,
        live,
        probe,
    } = prepared;
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
            access: Arc::clone(&access),
        },
        Arc::clone(&wiring.transport),
        Arc::clone(&cache),
        scope,
        Arc::clone(&roster),
        stopped.clone(),
    );

    let alerts = Arc::new(LogAlerts);
    let desk = CardDesk::new(
        DeskDeps {
            store: Arc::clone(&store),
            transport: Arc::clone(&wiring.transport),
            ids: RandomIds,
            clock: Arc::new(ApiClock(Arc::clone(&wiring.clock))),
            directory: roster.clone(),
            authority: Arc::new(StaffAuthority {
                roster: Arc::clone(&roster),
                access: Arc::clone(&access),
            }),
            alerts,
        },
        CardSettings {
            zone: config.runtime.timezone,
            policy: policy.clone(),
            instance_id: config.instance_id.clone(),
        },
    );
    let rsvp = ReactionRouter::new(
        StoreIndex(Arc::clone(&store)),
        SchedulerService::new(
            Arc::clone(&store),
            RandomIds,
            ApiClock(Arc::clone(&wiring.clock)),
        )
        .with_attendance(policy.attendance),
    );
    let (reaction_jobs, reaction_queue) = mpsc::unbounded_channel();

    let (guild_ready, ready) = watch::channel(false);
    let tick = TickLoop {
        store: Arc::clone(&store),
        transport: Arc::clone(&wiring.transport),
        cache: Arc::clone(&cache),
        roster: Arc::clone(&roster),
        clock: Arc::clone(&wiring.clock),
        period: wiring.tick,
        seeds: config.seeds.clone(),
        config: delivery_config(&config.instance_id, policy, &composition.settings),
        status: tick_status,
    };

    let dispatcher = commands::factory(
        Arc::clone(&composition.admin.state),
        Arc::clone(&store),
        Arc::clone(&cache),
        Arc::clone(&wiring.transport),
    )?;
    let ready_transport = Arc::clone(&wiring.transport);
    let owner_access = Arc::clone(&access);
    let mut handler = Fanout::new(
        scope.guild_id,
        Arc::clone(&wiring.transport),
        dispatcher,
        Arc::new(move || owner_access.owner()),
        roster,
        roster_jobs,
        reaction_jobs,
        Box::new(move |application| ready_transport.application_ready(application)),
        guild_ready,
    );
    let messages = handler.messages.clone();

    let workers = vec![
        tokio::spawn(roster_task.run(roster_queue)),
        tokio::spawn(Reactions { desk, rsvp }.run(reaction_queue)),
    ];
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
        tick: Some(tick),
        exit: None,
        messages,
        connection: probe.connection,
        steps: Vec::new(),
    })
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
                Ended::Shutdown => break,
                Ended::Gateway(exit) => {
                    self.gateway = None;
                    let fatal = matches!(exit, Ok(RunExit::Closed { .. }));
                    self.exit = Some(exit);
                    if fatal {
                        self.closed_for_good().await;
                        shutdown.await;
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
        if let Some(stop) = self.stop_gateway.take() {
            let _ = stop.send(());
        }
        if let Some(gateway) = self.gateway.take() {
            self.exit = Some(gateway.await.map_err(drop));
        }
        if !self.steps.contains(&"gateway_closed") {
            self.steps.push("gateway_closed");
            logging::event(
                "INFO",
                "gateway_closed",
                json!({"exit": format!("{:?}", self.exit)}),
            );
        }
        // The handler (and so every queue sender) is gone: workers drain.
        self.stop_workers.send_replace(true);
        for worker in self.workers.drain(..) {
            let _ = worker.await;
        }
        if !self.steps.contains(&"workers_stopped") {
            self.steps.push("workers_stopped");
        }
        if let Some(tick) = self.tick.take() {
            let _ = tick.await;
            self.steps.push("tick_stopped");
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
