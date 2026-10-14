//! The v5 scheduler service over a temp SQLite store: mutations, drafts,
//! proposals, requests, attendance and invariants.

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
