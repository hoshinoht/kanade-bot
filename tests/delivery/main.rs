//! The delivery executor and scheduler tick over the real journal (the
//! SQLite store) and the fake Discord: the exactly-once / no-replay
//! scenarios.

mod support;

mod attendance;
mod cards;
mod checkpoints;
mod debug;
mod debug_tools;
mod declines;
mod digest_v2;
mod drafts;
mod failures;
mod guides;
mod header_slots;
mod header_tease;
mod intercept;
mod late_start;
mod manual;
mod moves;
mod notices;
mod outage;
mod outbox_policy;
mod owner_requests;
mod pregen;
mod proposals;
mod redesign;
mod rewrite_log;
mod run_prompts;
mod scenarios;

// Pinned clock/ids and snapshots shared with other targets.
#[path = "../common/mod.rs"]
mod common;
