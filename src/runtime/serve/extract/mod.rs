//! Extraction in live serve: gateway messages → feed → `Pipeline` (debounce,
//! backlog, governed calls) → proposals and cards through the shared
//! `CardDesk`; the `Rescans` queue behind the API and `/rescan`; the startup
//! 24 h rescan; and the settings hook that switches it all live.
//!
//! Needs a model gateway with an extraction route; without one messages are
//! only counted and health reports `degraded` while the switch is on.
//! Self-service links: interim view links into the public portal (user
//! decision 2026-10-09), only with a public listener and while `self_service`
//! is open, read live.
//!
//! Stop (after the gateway handler, the feed's only sender, is gone):
//! `Rescans::close` (queued jobs cancelled, the running one stops before its
//! next burst), then the feed, pipeline, worker and hooks get [`STOP_GRACE`]
//! to finish; calls and permit waits still in flight are then cut (logged
//! `failed` with `CALL_CANCELLED`), and whatever outlives [`CANCEL_GRACE`] is
//! aborted. At most ~3 s plus the close's store writes.

mod ports;
mod runner;
mod status;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::{
    api::{
        admin::config::SettingsChanged,
        auth::Clock,
        rescan::{RescanRunner, RescanService},
        write::ApiClock,
    },
    bot::{
        cards::{CardDesk, CardOutbox},
        delivery::LogAlerts,
        extract_feed::{DiscordHistory, Feed, MessageFeed},
        guild_cache::GuildCache,
        roster::LiveRoster,
    },
    domain::{
        catalog::BossTable,
        ids::RandomIds,
        schedule::SchedulePolicy,
        settings::{RuntimeSettings, SelfService},
    },
    extract::{
        pipeline::{
            CallContext, DEFAULT_DEBOUNCE, DEFAULT_DRAIN_INTERVAL, Deps, Extractor, Pipeline,
            PipelineConfig, SelfServiceConfig, SelfServiceDeps,
        },
        redirect::{PortalViewLinks, SelfServiceMode},
        rescan::Rescans,
    },
    infrastructure::{
        llm::{governor::Role, setup::ModelStack},
        store::SqliteStore,
    },
    runtime::logging,
};

use super::budget::ShutdownClock;
use super::discord::GatewayTransport;
use super::tick::watch_list;
use ports::{ExtractorCache, LiveGuild, StoreProposer};
pub use runner::STARTUP_WINDOW;
use runner::{Gated, Startup};
pub use status::{ExtractionStatus, switched_on};

/// Pipeline events queued between the feed and the pipeline.
const EVENT_QUEUE: usize = 1_024;
/// How long stopping lets calls in flight finish before cutting them.
pub const STOP_GRACE: Duration = Duration::from_secs(1);
/// After the cut: log writes, then anything left is aborted.
pub const CANCEL_GRACE: Duration = Duration::from_secs(2);

/// Await every task; a task is dropped from `tasks` only once it finished,
/// so a timed-out call can resume.
async fn settle(tasks: &mut Vec<JoinHandle<()>>) {
    while let Some(task) = tasks.last_mut() {
        let _ = task.await;
        tasks.pop();
    }
}

/// The debounce and backlog pace (v4 defaults; tests shorten them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub debounce: Duration,
    pub drain_interval: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            debounce: DEFAULT_DEBOUNCE,
            drain_interval: DEFAULT_DRAIN_INTERVAL,
        }
    }
}

pub type Desk<T> = CardDesk<SqliteStore, T, RandomIds, LogAlerts>;

/// Everything extraction is built from.
pub struct Inputs<T> {
    pub store: Arc<SqliteStore>,
    pub models: Option<Arc<ModelStack>>,
    pub settings: RuntimeSettings,
    /// The config desk's saved changes; `None` never switches live.
    pub changes: Option<watch::Receiver<SettingsChanged>>,
    pub desk: Arc<Desk<T>>,
    pub transport: Arc<T>,
    pub cache: Arc<GuildCache>,
    pub roster: Arc<LiveRoster>,
    pub bosses: Arc<BossTable>,
    pub policy: SchedulePolicy,
    pub zone: chrono_tz::Tz,
    pub clock: Clock,
    pub status: Arc<ExtractionStatus>,
    pub timing: Timing,
    /// Flips once the guild is available (startup rescan).
    pub guild_ready: watch::Receiver<bool>,
    /// The public portal origin (`https://host`) when a public listener is
    /// configured; cards link into it.
    pub portal: Option<String>,
}

/// The planner's view of the saved `self_service` section.
fn self_service_config(saved: &SelfService) -> SelfServiceConfig {
    SelfServiceConfig {
        mode: SelfServiceMode::parse(saved.mode.as_str()).unwrap_or_default(),
        public_portal_open: saved.public_portal,
    }
}

/// View links into the portal, without lead-ins: there is no Move page yet,
/// so no weekly tip is spent on them.
fn self_service(portal: Option<&str>) -> Option<SelfServiceDeps> {
    let links = PortalViewLinks::new(portal?).ok()?;
    Some(SelfServiceDeps {
        links: Arc::new(links),
        lead_ins: None,
    })
}

type Close = Box<dyn FnOnce() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// The running extraction side.
pub struct Extraction {
    /// For the gateway handler; taken once.
    pub feed: Option<MessageFeed>,
    /// The API's and `/rescan`'s runner; `None` without an extractor.
    pub rescans: Option<Arc<dyn RescanRunner>>,
    stop: watch::Sender<bool>,
    feed_task: Option<JoinHandle<()>>,
    pipeline: Option<JoinHandle<()>>,
    startup: Option<JoinHandle<()>>,
    close: Option<Close>,
    /// Cuts the extractor's calls in flight.
    cancel: Option<Box<dyn Fn() + Send + Sync>>,
    worker: Option<JoinHandle<()>>,
    settings: Option<JoinHandle<()>>,
}

/// Run when extraction is switched off: cut calls, end rescans.
type SwitchedOff = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub fn start<T: GatewayTransport>(mut inputs: Inputs<T>) -> Extraction {
    let (stop, stopped) = watch::channel(false);
    inputs.status.set_enabled(switched_on(&inputs.settings));
    let (status, cache, halted) = (
        Arc::clone(&inputs.status),
        Arc::clone(&inputs.cache),
        stopped.clone(),
    );
    let follow_with =
        move |hook: Option<SwitchedOff>, changes: Option<watch::Receiver<SettingsChanged>>| {
            changes.map(|changes| tokio::spawn(follow(changes, status, cache, hook, halted)))
        };
    let changes = inputs.changes.take();
    let mut extraction = Extraction {
        feed: None,
        rescans: None,
        stop,
        feed_task: None,
        pipeline: None,
        startup: None,
        close: None,
        cancel: None,
        worker: None,
        settings: None,
    };
    let Some(stack) = inputs
        .models
        .clone()
        .filter(|stack| stack.has_role(Role::Extraction))
    else {
        extraction.settings = follow_with(None, changes);
        logging::event(
            "INFO",
            "extraction_unavailable",
            json!({"reason": "no extraction model", "enabled": inputs.status.enabled()}),
        );
        return extraction;
    };
    inputs.status.set_composed();

    let mut tuning = PipelineConfig::new(
        inputs.zone,
        inputs.settings.schedule.reset_weekday,
        inputs.settings.schedule.reset_time,
    );
    tuning.debounce = inputs.timing.debounce;
    tuning.drain_interval = inputs.timing.drain_interval;
    // The configured level; the runner floors `off` per call.
    tuning.reasoning = stack.effort(Role::Extraction);
    let clock = Arc::new(ApiClock(Arc::clone(&inputs.clock)));
    let extractor = Arc::new(
        Extractor::new(
            Deps {
                store: Arc::clone(&inputs.store),
                client: Arc::clone(&stack.client),
                guild: Arc::new(LiveGuild {
                    status: Arc::clone(&inputs.status),
                    cache: Arc::clone(&inputs.cache),
                    roster: Arc::clone(&inputs.roster),
                    bosses: inputs.bosses,
                }),
                proposer: Arc::new(StoreProposer {
                    store: Arc::clone(&inputs.store),
                    clock: Arc::clone(&inputs.clock),
                    policy: inputs.policy,
                    directory: Arc::clone(&inputs.roster),
                }),
                outbox: Arc::new(CardOutbox(inputs.desk)),
                clock,
                ids: Box::new(RandomIds),
                self_service: self_service(inputs.portal.as_deref()),
            },
            tuning,
        )
        .with_live_context({
            let resolve = super::context::resolver(
                Arc::clone(&stack),
                super::context::changes_or(changes.clone(), &inputs.settings),
                Role::Extraction,
            );
            Arc::new(move |alias: &str| {
                let context = resolve(alias);
                CallContext {
                    window: context.window as usize,
                    reserve: context.reserve as usize,
                    source: context.source.as_str(),
                }
            })
        })
        .with_live_self_service({
            let saved = super::context::changes_or(changes.clone(), &inputs.settings);
            Arc::new(move || self_service_config(&saved.borrow().settings.self_service))
        }),
    );

    let (events, queue) = mpsc::channel(EVENT_QUEUE);
    extraction.pipeline = Some(tokio::spawn(
        Pipeline::new(Arc::clone(&extractor)).run(queue),
    ));
    let (feed, items) = MessageFeed::channel();
    extraction.feed = Some(feed);
    extraction.feed_task = Some(tokio::spawn(
        Feed {
            events,
            stale: ExtractorCache(Arc::clone(&extractor)),
        }
        .run(items),
    ));

    let cutting = Arc::clone(&extractor);
    extraction.cancel = Some(Box::new(move || cutting.cancel_calls()));
    let interrupting = Arc::clone(&extractor);
    let rescans = Arc::new(Rescans::new(
        extractor,
        Arc::new(DiscordHistory {
            transport: inputs.transport,
            cache: Arc::clone(&inputs.cache),
        }),
    ));
    let worker = Arc::clone(&rescans);
    extraction.worker = Some(tokio::spawn(async move {
        match worker.recover().await {
            Ok(0) => {}
            Ok(jobs) => logging::event("INFO", "rescans_interrupted", json!({"jobs": jobs})),
            Err(_) => logging::event("WARN", "rescans_recover_failed", json!({})),
        }
        worker.run().await;
    }));
    let ending = Arc::clone(&rescans);
    let hook: SwitchedOff = Arc::new(move || {
        interrupting.interrupt_calls();
        let ending = Arc::clone(&ending);
        Box::pin(async move { ending.switched_off().await })
    });
    extraction.settings = follow_with(Some(hook), changes);
    let closing = Arc::clone(&rescans);
    extraction.close = Some(Box::new(move || {
        Box::pin(async move { closing.close().await })
    }));
    let runner: Arc<dyn RescanRunner> = Arc::new(Gated {
        inner: Arc::new(RescanService::new(rescans)),
        status: Arc::clone(&inputs.status),
    });
    extraction.startup = Some(tokio::spawn(
        Startup {
            runner: Arc::clone(&runner),
            status: inputs.status,
            cache: inputs.cache,
            roster: inputs.roster,
            models: stack,
        }
        .run(inputs.guild_ready, stopped),
    ));
    extraction.rescans = Some(runner);
    extraction
}

/// Saved settings apply at once: the switch (`extract_enabled`, `paused`)
/// and the watch list. Switching off also cuts calls in flight and ends
/// queued and running rescans, so nothing reaches the model afterwards.
async fn follow(
    mut changes: watch::Receiver<SettingsChanged>,
    status: Arc<ExtractionStatus>,
    cache: Arc<GuildCache>,
    switched_off: Option<SwitchedOff>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stop| *stop) => return,
            changed = changes.changed() => if changed.is_err() {
                return;
            },
        }
        let settings = Arc::clone(&changes.borrow_and_update().settings);
        cache.set_watch(watch_list(&settings));
        let on = switched_on(&settings);
        if status.set_enabled(on) != on {
            logging::event("INFO", "extraction_switched", json!({"enabled": on}));
            if !on && let Some(switched_off) = &switched_off {
                switched_off().await;
            }
        }
    }
}

impl Extraction {
    /// Ordered stop (see the module docs); call after the gateway handler
    /// was dropped. Idempotent.
    pub async fn stop(&mut self, clock: &ShutdownClock) {
        self.stop.send_replace(true);
        // Nothing else may keep the feed or the store open.
        self.feed = None;
        self.rescans = None;
        // Queued jobs end now; the running one stops before its next burst.
        if let Some(close) = self.close.take() {
            close().await;
        }
        let mut running: Vec<JoinHandle<()>> = [
            self.settings.take(),
            self.startup.take(),
            self.worker.take(),
            self.pipeline.take(),
            self.feed_task.take(),
        ]
        .into_iter()
        .flatten()
        .collect();
        // Holds the extractor, and so the store.
        let cancel = self.cancel.take();
        if clock
            .bounded("extraction", STOP_GRACE, settle(&mut running))
            .await
            .is_some()
        {
            return;
        }
        // Compose kills the container 30 s after SIGTERM: cut the calls and
        // permit waits still in flight (each logged cancelled).
        if let Some(cancel) = cancel {
            cancel();
        }
        logging::event("WARN", "extraction_calls_cancelled", json!({}));
        if clock
            .bounded("extraction_cancel", CANCEL_GRACE, settle(&mut running))
            .await
            .is_none()
        {
            for task in &running {
                task.abort();
            }
            logging::event(
                "WARN",
                "extraction_aborted",
                json!({"tasks": running.len()}),
            );
            settle(&mut running).await;
        }
    }
}
