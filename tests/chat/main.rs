//! The chat surface on a synthetic world: authority and party checks, tool
//! schemas and read tools, proposals, reply shaping, v5's dynamic tool
//! bundles, the governed question loop and the chat log.

mod answer;
mod authority;
mod budget;
mod bundles;
mod duplicate_add;
mod guessed_run;
mod live_switch;
mod looping;
mod mixed_people;
mod model;
mod pilot;
mod profanity;
mod proposal_dedupe;
mod propose;
mod read_tools;
mod run_context;
mod sanitize;
mod slow_store;
mod staging;
mod strategy_guides;
mod support;
mod tool_schemas;
mod voice_mention;
mod wire;
mod world;

// Pinned clock/ids and JSON helpers shared with the scheduler target.
#[path = "../common/mod.rs"]
mod common;
