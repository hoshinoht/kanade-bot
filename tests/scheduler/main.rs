//! The v5 scheduler service over the in-memory store: mutations, drafts,
//! proposals, requests, attendance, store conformance and invariants.

mod attendance_v5;
mod draft_service;
mod drafts;
mod fixed_create;
mod frozen;
mod invariants;
mod mutations_v5;
mod proposal_follow_ups;
mod proposals;
mod requests;
mod rollback_previews;
mod support;

// Pinned clock/ids and snapshots shared with other targets.
#[path = "../common/mod.rs"]
mod common;

use kanade::infrastructure::store::{MemoryScheduleStore, conformance};

#[tokio::test]
async fn memory_store_passes_the_store_conformance_suite() {
    conformance::run_suite(async || MemoryScheduleStore::new()).await;
}
