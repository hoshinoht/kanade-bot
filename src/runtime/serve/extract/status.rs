//! The extraction switch the pipeline, rescans and health read.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::{
    domain::{
        model_log::{ExtractionFilter, ExtractionOutcome, ModelLogStore},
        settings::RuntimeSettings,
    },
    extract::pipeline::{CALL_CANCELLED, CALL_SWITCHED_OFF},
    infrastructure::store::SqliteStore,
};

/// `extract_enabled` and not `paused`, as last published; `composed` once
/// the extractor exists (a model gateway with an extraction route).
#[derive(Debug, Default)]
pub struct ExtractionStatus {
    enabled: AtomicBool,
    composed: AtomicBool,
}

pub fn switched_on(settings: &RuntimeSettings) -> bool {
    settings.watching.extract_enabled && !settings.watching.paused
}

impl ExtractionStatus {
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// The previous value.
    pub fn set_enabled(&self, on: bool) -> bool {
        self.enabled.swap(on, Ordering::SeqCst)
    }

    pub fn set_composed(&self) {
        self.composed.store(true, Ordering::SeqCst);
    }

    /// `disabled` (switched off), `degraded` (on, but no extractor, or the
    /// latest call failed, other than a shutdown cut, or was turned away),
    /// `running` (a rescan is queued
    /// or running) or `idle`. A store read failure skips its check.
    pub async fn state(&self, store: &SqliteStore) -> &'static str {
        if !self.enabled() {
            return "disabled";
        }
        if !self.composed.load(Ordering::SeqCst) {
            return "degraded";
        }
        if let Ok(jobs) = store.recent_rescan_jobs(5).await
            && jobs.iter().any(|job| !job.status.is_final())
        {
            return "running";
        }
        let latest = ExtractionFilter {
            limit: 1,
            omit_bodies: true,
            ..ExtractionFilter::default()
        };
        if let Ok(page) = store.list_extractions(&latest).await
            && page.items.first().is_some_and(|log| {
                matches!(
                    log.outcome,
                    ExtractionOutcome::Failed | ExtractionOutcome::TurnedAway
                ) && !matches!(
                    log.error.as_deref(),
                    Some(CALL_CANCELLED | CALL_SWITCHED_OFF)
                )
            })
        {
            return "degraded";
        }
        "idle"
    }
}
