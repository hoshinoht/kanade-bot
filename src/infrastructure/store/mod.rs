//! Scheduler store and delivery-journal adapters. SQLite is the production
//! store; conformance suites run against SQLite in test support.

pub mod auth_audit;
mod history;
pub mod maintenance;
mod observer;
mod order;
pub mod replays;
mod rules;
pub mod snapshots;
pub mod sqlite;
pub mod web_sessions;

#[cfg(any(test, feature = "test-support"))]
pub mod attendance_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod auth_audit_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod card_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod characters_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod chardata_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod cherry_pick_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod decline_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod draft_conformance;
#[cfg(any(test, feature = "test-support"))]
mod fixtures;
#[cfg(any(test, feature = "test-support"))]
pub mod guides_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod hints_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod history_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod journal_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod members_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod model_log_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod outbox_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod owner_request_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod picks_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod precondition_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod proposal_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod replay_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod run_prompt_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod settings_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod web_sessions_conformance;

pub use observer::{RunObserver, WriteObserver, Written};
#[cfg(any(test, feature = "test-support"))]
pub use sqlite::TempStoreDir;
pub use sqlite::{
    BackupManifest, Seal, SqliteCharData, SqliteStore, SqliteStoreConfig, SqliteStoreError,
    SwapOwnership,
};
