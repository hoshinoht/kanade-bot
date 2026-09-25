//! The rescan runner the API and `/rescan` share, refused while extraction
//! is switched off, and the automated startup rescan.

use std::sync::Arc;

use serde_json::json;
use tokio::sync::watch;

use super::status::ExtractionStatus;
use crate::{
    api::rescan::{RescanFuture, RescanRunner, RescanView},
    bot::{commands::GuildChannels, guild_cache::GuildCache, roster::LiveRoster},
    domain::members::MemberStore,
    extract::rescan::{RescanError, RescanRequest},
    infrastructure::store::SqliteStore,
    runtime::logging,
};

/// The startup rescan's window (automated jobs are capped at 48 h anyway).
pub const STARTUP_WINDOW: &str = "24h";

/// While extraction is off a submit is refused as `Closed` (`/rescan`:
/// "not available"; API: `503`), so a switched-off bot never calls the
/// model. Reads and cancels always pass.
pub struct Gated {
    pub inner: Arc<dyn RescanRunner>,
    pub status: Arc<ExtractionStatus>,
}

impl RescanRunner for Gated {
    fn submit(&self, request: RescanRequest) -> RescanFuture<'_, RescanView> {
        if !self.status.enabled() {
            return Box::pin(async { Err(RescanError::Closed) });
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

pub struct Startup {
    pub runner: Arc<dyn RescanRunner>,
    pub status: Arc<ExtractionStatus>,
    pub cache: Arc<GuildCache>,
    pub roster: Arc<LiveRoster>,
    pub store: Arc<SqliteStore>,
}

impl Startup {
    /// Once the guild is available: re-read the last 24 h of every watched
    /// channel, if extraction is on.
    pub async fn run(self, mut ready: watch::Receiver<bool>, mut stop: watch::Receiver<bool>) {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stop| *stop) => return,
            ready = ready.wait_for(|ready| *ready) => if ready.is_err() {
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
        // The roster task may not have reconciled yet; the stored rows say
        // who holds the bossing role meanwhile.
        if let Ok(rows) = self.store.list_members().await {
            self.roster.replace(rows);
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
                    RescanError::Store(_) => "store",
                }}),
            ),
        }
    }
}
