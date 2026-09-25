//! The Discord adapter on Twilight: an outcome-classifying transport seam,
//! the explicit mention policy, a single-shard gateway runner, event mapping
//! and the slash-command framework.
//!
//! Storage-independent: card lookups, roster persistence and the delivery
//! journal are ports implemented by the store. Nothing here connects to
//! Discord unless a caller hands the runner a real shard.

pub mod cards;
pub mod chat_feed;
pub mod commands;
pub mod delivery;
pub mod events;
pub mod extract_feed;
pub mod gateway;
pub mod guild_cache;
pub mod handler;
pub mod identity;
pub mod ids;
pub mod mentions;
pub mod roster;
pub mod transport;
