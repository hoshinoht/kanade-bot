//! The rescan runner the API and `/rescan` share, refused while extraction
//! is switched off, and the automated startup rescan.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;

use super::status::ExtractionStatus;
use crate::{
    api::rescan::{RescanFuture, RescanRunner, RescanView},
    bot::{commands::GuildChannels, guild_cache::GuildCache, roster::LiveRoster},
    extract::rescan::{RescanError, RescanRequest},
    infrastructure::llm::setup::ModelStack,
    runtime::logging,
};

/// The startup rescan's window (automated jobs are capped at 48 h anyway).
pub const STARTUP_WINDOW: &str = "24h";

/// While extraction is off a submit is refused as `Off` (`/rescan` and the
/// API say to switch it on), so a switched-off bot never calls the
/// model. Reads and cancels always pass.
pub struct Gated {
    pub inner: Arc<dyn RescanRunner>,
    pub status: Arc<ExtractionStatus>,
}

impl RescanRunner for Gated {
    fn ready(&self) -> Result<(), RescanError> {
        if self.status.enabled() {
            self.inner.ready()
        } else {
            Err(RescanError::Off)
        }
    }

    fn submit(&self, request: RescanRequest) -> RescanFuture<'_, RescanView> {
        if let Err(error) = self.ready() {
            return Box::pin(async { Err(error) });
        }
        self.inner.submit(request)
    }

    fn job(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        self.inner.job(id)
    }

    fn cancel(&self, id: String) -> RescanFuture<'_, Option<RescanView>> {
        self.inner.cancel(id)
    }
}

/// How long the startup rescan waits for the first model listing and the
/// first roster reconcile before going ahead without them.
const STARTUP_WAIT: Duration = Duration::from_secs(60);
const LISTING_POLL: Duration = Duration::from_millis(200);

pub struct Startup {
    pub runner: Arc<dyn RescanRunner>,
    pub status: Arc<ExtractionStatus>,
    pub cache: Arc<GuildCache>,
    pub roster: Arc<LiveRoster>,
    pub models: Arc<ModelStack>,
}

impl Startup {
    /// Once the guild is available, the model listing has named the routes'
    /// trust zones and the roster has reconciled: read the last 24 h of every
    /// watched channel that no pass has read yet, if extraction is on.
    pub async fn run(self, mut ready: watch::Receiver<bool>, mut stop: watch::Receiver<bool>) {
        let prepared = async {
            if ready.wait_for(|ready| *ready).await.is_err() {
                return false;
            }
            let listed = async {
                while !self.models.catalog().listed {
                    tokio::time::sleep(LISTING_POLL).await;
                }
            };
            let _ = tokio::time::timeout(STARTUP_WAIT, async {
                listed.await;
                self.roster.reconciled().await;
            })
            .await;
            true
        };
        tokio::select! {
            biased;
            _ = stop.wait_for(|stop| *stop) => return,
            prepared = prepared => if !prepared {
                return;
            },
        }
        if !self.status.enabled() {
            logging::event(
                "INFO",
                "startup_rescan_skipped",
                json!({"reason": "disabled"}),
            );
            return;
        }
        let channels = GuildChannels::watched(&*self.cache);
        if channels.is_empty() {
            logging::event(
                "INFO",
                "startup_rescan_skipped",
                json!({"reason": "no_channels"}),
            );
            return;
        }
        let count = channels.len();
        let request = RescanRequest {
            channels,
            window: STARTUP_WINDOW.to_owned(),
            source: "startup".to_owned(),
            automated: true,
            requested_by: None,
            unprocessed_only: true,
        };
        match self.runner.submit(request).await {
            Ok(view) => logging::event(
                "INFO",
                "startup_rescan_queued",
                json!({"job": view.job.id, "channels": count}),
            ),
            // Store text can carry paths; the kind is enough.
            Err(error) => logging::event(
                "WARN",
                "startup_rescan_failed",
                json!({"kind": match error {
                    RescanError::NoChannels => "no_channels",
                    RescanError::Window(_) => "window",
                    RescanError::Closed => "closed",
                    RescanError::Off => "off",
                    RescanError::Store(_) => "store",
                }}),
            ),
        }
    }
}
