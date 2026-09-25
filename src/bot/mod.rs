//! The Discord adapter on Twilight: an outcome-classifying transport seam,
//! the explicit mention policy, a single-shard gateway runner, event mapping
//! and the slash-command framework.
//!
//! Storage-independent: card lookups, roster persistence and the delivery
//! journal are ports implemented later. Nothing here connects to Discord
//! unless a caller hands the runner a real shard.

pub mod cards;
pub mod commands;
pub mod delivery;
pub mod events;
pub mod gateway;
pub mod ids;
pub mod mentions;
pub mod transport;
