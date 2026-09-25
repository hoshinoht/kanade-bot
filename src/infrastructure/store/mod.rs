//! Scheduler store and delivery-journal adapters. SQLite is the production
//! store; the in-memory store and the conformance suites every store must
//! pass are test support.

mod history;
mod order;
pub mod sqlite;
pub mod web_sessions;

#[cfg(any(test, feature = "test-support"))]
pub mod attendance_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod cherry_pick_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod draft_conformance;
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
pub mod precondition_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod proposal_conformance;
#[cfg(any(test, feature = "test-support"))]
pub mod web_sessions_conformance;

#[cfg(any(test, feature = "test-support"))]
pub use memory::MemoryScheduleStore;
pub use sqlite::{BackupManifest, SqliteStore, SqliteStoreConfig, SqliteStoreError};
