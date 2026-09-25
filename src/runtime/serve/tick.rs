//! The delivery tick loop: recovery once, then one tick every
//! `KANADE_TICK_SECONDS` under its own lease. The outbox drain, reminders,
//! digests and expiry all run inside `Delivery::tick_at` in v4's order.
//! A tick is never cancelled midway: stopping waits for the running one.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};

use super::settings;
use crate::{
    api::auth::Clock,
    bot::{
        delivery::{
            DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, DeliveryError, LogAlerts,
            TickReport,
        },
        guild_cache::{GuildCache, WatchList},
        ids::parse_id,
        roster::LiveRoster,
        transport::DiscordTransport,
    },
    domain::{
        ids::RandomIds, members::MemberStore, notify::DEFAULT_MAX_NOTICE_AGE,
        settings::RuntimeSettings,
    },
    infrastructure::store::SqliteStore,
    runtime::{config::SettingSeeds, logging},
};

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
    /// Wait for the guild, recover in-flight attempts, then tick until
    /// `stop`. Returns after the running tick completes.
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
        let alerts = LogAlerts;
        let mut delivery = Delivery::new(
            &*self.store,
            RandomIds,
            &*self.transport,
            &alerts,
            &*self.roster,
            &*self.cache,
            self.config.clone(),
        );
        let mut interval = tokio::time::interval(self.period);
        interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut recovered = false;
        loop {
            tokio::select! {
                biased;
                _ = stop.wait_for(|stop| *stop) => break,
                _ = interval.tick() => {}
            }
            // Recovery must succeed before any send: until then a previous
            // process's in-flight attempts are not yet marked indeterminate.
            if !recovered {
                match delivery.start((self.clock)()).await {
                    Ok(recovery) => {
                        recovered = true;
                        logging::event(
                            "INFO",
                            "delivery_recovered",
                            json!({
                                "indeterminate": recovery.indeterminate.len(),
                                "orphaned_leases": recovery.orphaned_leases,
                            }),
                        );
                        self.status.set(RUNNING);
                    }
                    Err(error) => {
                        tick_failed("recover", &error);
                        continue;
                    }
                }
            }
            self.refresh(&mut delivery.config).await;
            match delivery.tick_at((self.clock)()).await {
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
            self.cache.set_watch(watch_list(&settings));
        }
        if let Ok(rows) = self.store.list_members().await {
            self.roster.replace(rows);
        }
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
