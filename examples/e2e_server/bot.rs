//! Serve's Discord side (`discord::start`: chat, extraction and its rescans,
//! the card desk, the manual header rewrite, header pre-generation and the
//! tick), over the crate's fake Discord transport and a gateway that never
//! connects. The guild therefore never becomes ready: the tick, the roster
//! sync and the startup rescan never run, and the fake Discord is only
//! called for what an admin starts (a rescan pages its empty history, a
//! header rewrite edits its messages).
//!
//! `Discord` is not `Send` (its tick is polled, not spawned), so it lives on
//! a blocking-pool thread that drives it until [`Bot::stop`]; everything it
//! spawns runs on the server's runtime.

use std::{future::pending, sync::Arc, time::Duration};

use kanade::{
    app::clock::Clock,
    bot::gateway::{EventSource, GatewayError},
    discord::transport::FakeDiscord,
    infrastructure::store::SqliteStore,
    runtime::{
        config::ServeConfig,
        serve::{
            bot::{
                discord::{self, Wiring},
                extract::Timing,
            },
            composition::Composition,
        },
    },
};
use tokio::{sync::oneshot, task::JoinHandle};
use twilight_gateway::Event;

use crate::Fail;

/// Serve's tick period; the tick waits for a guild that never comes.
const TICK: Duration = Duration::from_secs(60);
/// One burst per half second (serve: 15 s), so a rescan of the seeded
/// channels runs for a few seconds, long enough to be seen running.
const EXTRACTION: Timing = Timing {
    debounce: kanade::extract::pipeline::DEFAULT_DEBOUNCE,
    drain_interval: Duration::from_millis(500),
};

/// A gateway that never delivers an event (no `READY`, no guild).
struct Silent(bool);

impl EventSource for Silent {
    async fn next_event(&mut self) -> Option<Result<Event, GatewayError>> {
        if self.0 {
            return None;
        }
        pending().await
    }

    fn close(&mut self) {
        self.0 = true;
    }
}

/// The running Discord side of one generation.
pub struct Bot {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl Bot {
    /// Start the Discord side over `composition`, whose state nothing else
    /// holds yet (it attaches the rescan runner, the header rewrite and the
    /// card refresh to it), and hand the composition back. `transport` is
    /// the generation's fake Discord (shared with the guide publisher and
    /// `/__e2e/discord/*`).
    pub async fn start(
        config: ServeConfig,
        store: Arc<SqliteStore>,
        mut composition: Composition,
        clock: Clock,
        transport: Arc<FakeDiscord>,
    ) -> Result<(Composition, Self), Fail> {
        let runtime = tokio::runtime::Handle::current();
        let (started, on_start) = oneshot::channel();
        let (stop, stopped) = oneshot::channel::<()>();
        let task = tokio::task::spawn_blocking(move || {
            runtime.block_on(async move {
                let prepared = discord::prepare(&config, TICK);
                let wiring = Wiring {
                    source: Silent(false),
                    transport,
                    clock,
                    tick: TICK,
                    extraction: EXTRACTION,
                };
                match discord::start(&config, store, &mut composition, prepared, wiring).await {
                    Ok(mut side) => {
                        let _ = started.send(Ok(composition));
                        side.until(async {
                            let _ = stopped.await;
                        })
                        .await;
                    }
                    Err(error) => {
                        let _ = started.send(Err(error));
                    }
                }
            });
        });
        match on_start.await {
            Ok(Ok(composition)) => Ok((composition, Self { stop, task })),
            Ok(Err(error)) => {
                let _ = task.await;
                Err(error.into())
            }
            Err(_) => {
                let _ = task.await;
                Err("the Discord side ended before it started".into())
            }
        }
    }

    /// Stop it in serve's order and wait until everything it ran has ended.
    pub async fn stop(self) {
        let _ = self.stop.send(());
        if let Err(error) = self.task.await {
            eprintln!("e2e_server: the Discord side failed: {error}");
        }
    }
}
