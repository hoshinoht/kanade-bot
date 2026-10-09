//! The delivery executor and scheduler tick over the real journal (memory
//! and SQLite stores) and the fake Discord: vector replays plus the
//! exactly-once / no-replay scenarios.

mod support;

mod attendance;
mod cards;
mod checkpoints;
mod debug;
mod debug_tools;
mod declines;
mod digest_replay;
mod digest_v2;
mod dispatch_replay;
mod drafts;
mod failures;
mod intercept;
mod manual;
mod notices;
mod outage;
mod outbox_policy;
mod owner_requests;
mod pregen;
mod proposals;
mod redesign;
mod rewrite_log;
mod scenarios;

// Vector loading, pinned clock/ids and snapshots shared with other targets.
#[path = "../common/mod.rs"]
mod common;
