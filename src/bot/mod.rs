//! The Discord bot on Twilight's gateway: a single-shard runner, event
//! mapping, the slash-command framework, delivery, cards, chat and
//! extraction feeds and the guide publisher.
//!
//! Storage-independent: card lookups, roster persistence and the delivery
//! journal are ports implemented by the store. Nothing here connects to
//! Discord unless a caller hands the runner a real shard.

pub mod cards;
pub mod chardata;
pub mod chat_feed;
pub mod commands;
pub mod delivery;
pub mod events;
pub mod extract_feed;
pub mod gateway;
pub mod guides;
pub mod handler;
pub(crate) mod reaction_read;
pub mod roster;
pub mod rsvp_replay;
