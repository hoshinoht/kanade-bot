//! Scheduler store and delivery-journal adapters. SQLite is the production
//! store; the in-memory store and the conformance suites every store must
//! pass are test support.

pub mod auth_audit;
mod history;
mod observer;
mod order;
pub mod replays;
pub mod sqlite;
pub mod web_sessions;

#[cfg(any(test, feature = "test-support"))]
pub mod attendance_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod auth_audit_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod card_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod cherry_pick_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod decline_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod draft_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod hints_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod history_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod journal_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod members_conformance;
#[cfg(any(test, feature = "test-support"))]
mod memory;
#[cfg(any(test, feature = "test-support"))]
pub mod model_log_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod outbox_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod precondition_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod proposal_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod replay_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod settings_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod web_sessions_conformance;

#[cfg(any(test, feature = "test-support"))]
pub use memory::MemoryScheduleStore;
pub use observer::{RunObserver, WriteObserver, Written};
pub use sqlite::{BackupManifest, SqliteStore, SqliteStoreConfig, SqliteStoreError};
