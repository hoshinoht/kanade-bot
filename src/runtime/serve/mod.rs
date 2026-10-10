//! Live `serve`: owns the SQLite store, serves the admin and public routers
//! against it and, unless `KANADE_DISCORD_GATEWAY=0`, runs the Discord
//! gateway, roster sync, extraction and delivery tick for the configured
//! guild.

pub mod api;
pub mod chat;
mod chat_cards;
mod chat_log;
mod commands;
mod context;
pub mod discord;
pub mod extract;
#[cfg(feature = "test-support")]
pub mod harness;
mod health;
pub mod models;
mod persona_log;
mod sessions;
pub mod settings;
mod store;
mod tick;

use std::{future::Future, sync::Arc, time::Duration};

use crate::{
    api::{auth, server, state::StaticChannels},
    bot::gateway::{EventSource, shard},
    infrastructure::store::SqliteStore,
    runtime::{
        backup::crypt::Recipients,
        config::{BACKUP_RECIPIENTS_KEY, ServeConfig},
        error::Error,
        logging,
    },
};

use discord::{GatewayTransport, LateTransport, Wiring};
use health::LiveHealth;

/// How long closing waits for connections a timed-out drain left behind.
const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// One shutdown deadline, started when the shutdown signal fires; every
/// phase gets only what remains of it.
pub mod budget {
    use std::{sync::Arc, time::Duration};

    use serde_json::json;
    use tokio::{sync::watch, time::Instant};

    use crate::{infrastructure::store::SqliteStore, runtime::logging};

    /// Total from the signal to `serve` returning: deploy/compose.yaml
    /// `stop_grace_period` (30 s) minus a 5 s margin. Keep them coupled.
    /// The margin also absorbs the one accepted overrun: a tick write begun
    /// just before the cutoff waiting out SQLite's 5 s `busy_timeout` on a
    /// lock an outside process holds (about 27 s).
    pub const TOTAL: Duration = Duration::from_secs(25);
    /// Kept back at the end for the store close, which always runs. Chat's
    /// stop may spend up to 1 s of it on aborted questions' log writes
    /// (`chat::driver::LOG_BUDGET`), so the close starts with at least
    /// `STORE_RESERVE - LOG_BUDGET` (2 s) left. Phases after chat (the
    /// HTTP drain included) then get nothing and are cut at once; an HTTP
    /// request still in flight keeps its connection task and the store
    /// close waits for it within what is left (its store reads fail at once
    /// from the cutoff on: [`ShutdownClock::refusing_reads`]).
    pub const STORE_RESERVE: Duration = Duration::from_secs(3);
    /// Discord calls still in flight are cut this long before the phases'
    /// share ends, so the journal outcome each cut send records (a short
    /// write) lands before workers are aborted and reads are refused.
    pub const SEND_FINALISE: Duration = Duration::from_secs(1);

    /// What a phase with its own `grace` may take `elapsed` after the
    /// signal: never past `TOTAL - STORE_RESERVE`.
    pub fn allot(elapsed: Duration, grace: Duration) -> Duration {
        grace.min((TOTAL - STORE_RESERVE).saturating_sub(elapsed))
    }

    /// What the store close may take `elapsed` after the signal.
    pub fn store_allot(elapsed: Duration, wait: Duration) -> Duration {
        wait.min(TOTAL.saturating_sub(elapsed))
    }

    /// Shared start of the deadline; unstarted (a fatal gateway close
    /// stopping the Discord side early) leaves every phase its own grace.
    #[derive(Clone)]
    pub struct ShutdownClock(Arc<watch::Sender<Option<Instant>>>);

    impl Default for ShutdownClock {
        fn default() -> Self {
            Self(Arc::new(watch::channel(None).0))
        }
    }

    impl ShutdownClock {
        /// Idempotent: the first call fixes the deadline.
        pub fn start(&self) {
            self.0.send_if_modified(|started| {
                let first = started.is_none();
                started.get_or_insert_with(Instant::now);
                first
            });
        }

        /// Test-only [`Self::start`] as of `at` (a phase that already ate
        /// most of the budget).
        #[cfg(test)]
        pub fn start_at(&self, at: Instant) {
            self.0
                .send_if_modified(|started| started.get_or_insert(at) == &at);
        }

        fn elapsed(&self) -> Option<Duration> {
            self.0.borrow().map(|started| started.elapsed())
        }

        /// Resolves when the phases' share of the deadline is spent (never
        /// before the clock starts).
        pub async fn expired(&self) {
            self.after(TOTAL - STORE_RESERVE).await;
        }

        /// Resolves [`SEND_FINALISE`] before [`Self::expired`]: when Discord
        /// calls in flight are cut (never before the clock starts).
        pub async fn sends_ended(&self) {
            self.after(TOTAL - STORE_RESERVE - SEND_FINALISE).await;
        }

        async fn after(&self, offset: Duration) {
            let mut started = self.0.subscribe();
            let Ok(at) = started.wait_for(Option::is_some).await.map(|at| *at) else {
                return std::future::pending().await;
            };
            if let Some(at) = at {
                tokio::time::sleep_until(at + offset).await;
            }
        }

        /// When the phases' share ends; `None` before the clock starts.
        pub fn phase_end(&self) -> Option<Instant> {
            self.0
                .borrow()
                .map(|started| started + (TOTAL - STORE_RESERVE))
        }

        pub fn phase(&self, grace: Duration) -> Duration {
            self.elapsed()
                .map_or(grace, |elapsed| allot(elapsed, grace))
        }

        pub fn store(&self, wait: Duration) -> Duration {
            self.elapsed()
                .map_or(wait, |elapsed| store_allot(elapsed, wait))
        }

        /// Whether the deadline, not the phase's own grace, limits `grace`.
        pub fn cuts(&self, grace: Duration) -> bool {
            self.phase(grace) < grace
        }

        /// One structured line per phase the deadline cut.
        pub fn cut(&self, phase: &'static str) {
            let elapsed = self.elapsed().unwrap_or_default();
            logging::event(
                "WARN",
                "shutdown_deadline_cut",
                json!({"phase": phase, "elapsed_ms": elapsed.as_millis() as u64}),
            );
        }

        /// `work`, during which the store refuses new reads once the
        /// phases' share is spent: past the cutoff, every read still to
        /// come (the running tick's above all) fails at once instead of
        /// each waiting up to the reader acquire timeout. Inert until the
        /// clock starts; writes are untouched.
        pub async fn refusing_reads<F: Future>(&self, store: &SqliteStore, work: F) -> F::Output {
            let cutoff = async {
                self.expired().await;
                store.refuse_reads();
                logging::event("WARN", "store_reads_refused", json!({}));
                std::future::pending::<()>().await;
            };
            tokio::select! {
                output = work => output,
                () = cutoff => unreachable!("the cutoff never ends"),
            }
        }

        /// Run `work` for at most `grace` (less if the deadline is nearer);
        /// `None` (and a cut line when the deadline was the limit) if cut.
        pub async fn bounded<F: Future>(
            &self,
            phase: &'static str,
            grace: Duration,
            work: F,
        ) -> Option<F::Output> {
            let limit = self.phase(grace);
            let done = tokio::time::timeout(limit, work).await.ok();
            if done.is_none() && limit < grace {
                self.cut(phase);
            }
            done
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn phases_share_one_deadline_and_keep_the_store_reserve() {
            let s = Duration::from_secs;
            assert_eq!(allot(s(0), s(5)), s(5));
            assert_eq!(allot(s(20), s(5)), s(2));
            assert_eq!(allot(s(22), s(5)), s(0));
            assert_eq!(allot(s(30), s(5)), s(0));
            assert_eq!(allot(s(1), Duration::MAX), s(21));
            assert_eq!(store_allot(s(22), s(5)), s(3));
            assert_eq!(store_allot(s(10), s(5)), s(5));
            assert_eq!(store_allot(s(26), s(5)), s(0));
            const { assert!(TOTAL.as_secs() + 5 <= 30) };
        }
    }
}

pub async fn run(config: ServeConfig) -> Result<(), Error> {
    serve_until(config, server::wait_for_shutdown()).await
}

/// [`run`] with the shutdown signal supplied (tests).
pub async fn serve_until(
    config: ServeConfig,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    // Checked now so a missing secret fails the deploy, not the gateway later.
    let token = config.discord.read_token()?;
    // Likewise a broken backup key file fails the deploy, not the next backup.
    if let Some(path) = &config.backup_recipients_file {
        Recipients::load(path, BACKUP_RECIPIENTS_KEY)?;
    }
    if !config.discord.gateway {
        drop(token);
        let store = store::open(&config.store).await?;
        let clock = budget::ShutdownClock::default();
        let served = serve_store(&config, store.clone(), shutdown, &clock).await;
        store::close(store, clock.store(CLOSE_WAIT)).await;
        return served;
    }
    // One gateway session per token: v4 must be stopped before this connects.
    config.discord.require_v4_stopped()?;
    let wiring = Wiring {
        source: shard(token.expose().to_owned()),
        transport: Arc::new(LateTransport::new(token)),
        clock: Arc::new(auth::system_now),
        tick: config.tick,
        extraction: extract::Timing::default(),
    };
    serve_with(&config, shutdown, wiring).await
}

/// Live serve over any gateway source and transport; closes the store it
/// opens, after everything that used it has stopped.
pub async fn serve_with<S, T>(
    config: &ServeConfig,
    shutdown: impl Future<Output = ()>,
    wiring: Wiring<S, T>,
) -> Result<(), Error>
where
    S: EventSource + 'static,
    T: GatewayTransport,
{
    let store = store::open(&config.store).await?;
    let clock = budget::ShutdownClock::default();
    let served = serve_live(config, store.clone(), shutdown, wiring, &clock).await;
    store::close(store, clock.store(CLOSE_WAIT)).await;
    served
}

async fn serve_store(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    shutdown: impl Future<Output = ()>,
    clock: &budget::ShutdownClock,
) -> Result<(), Error> {
    // No guild cache without the gateway: channel pickers are empty.
    let health = LiveHealth::new(store.clone());
    let composition =
        api::compose(config, store, Arc::new(StaticChannels(Vec::new())), health).await?;
    logging::discord_disabled();
    let prune =
        sessions::SessionPrune::start(composition.admin.member.clone(), sessions::PRUNE_EVERY);
    let shutdown = async {
        shutdown.await;
        clock.start();
        clock.clone()
    };
    let served = server::serve_bounded(&config.runtime, Some(composition.admin), shutdown).await;
    if let Some(prune) = prune {
        prune.stop().await;
    }
    served
}

async fn serve_live<S, T>(
    config: &ServeConfig,
    store: Arc<SqliteStore>,
    shutdown: impl Future<Output = ()>,
    wiring: Wiring<S, T>,
    clock: &budget::ShutdownClock,
) -> Result<(), Error>
where
    S: EventSource + 'static,
    T: GatewayTransport,
{
    let mut prepared = discord::prepare(config, wiring.tick);
    prepared.shutdown = clock.clone();
    let health = LiveHealth::new(store.clone())
        .with_discord(prepared.probe.clone(), prepared.tick_status.clone())
        .with_extraction(prepared.extraction.clone());
    let mut composition =
        api::compose(config, store.clone(), prepared.cache.clone(), health).await?;
    let mut discord =
        discord::start(config, store.clone(), &mut composition, prepared, wiring).await?;
    // HTTP keeps serving until the Discord side has stopped, then drains.
    let stopped = async {
        discord.until(shutdown).await;
        clock.clone()
    };
    let prune =
        sessions::SessionPrune::start(composition.admin.member.clone(), sessions::PRUNE_EVERY);
    let served = clock
        .refusing_reads(
            &store,
            server::serve_bounded(&config.runtime, Some(composition.admin), stopped),
        )
        .await;
    if let Some(prune) = prune {
        prune.stop().await;
    }
    discord.stop().await;
    discord.result().and(served)
}

#[cfg(test)]
mod chat_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
