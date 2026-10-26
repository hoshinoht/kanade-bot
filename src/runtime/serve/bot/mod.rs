//! The `bot` component's wiring: the Discord gateway and its REST side, the
//! slash commands, chat, extraction and the delivery tick, and the ports it
//! attaches to the web side's state.

pub mod chat;
pub(super) mod chat_cards;
pub(super) mod commands;
pub mod discord;
pub mod extract;
pub(super) mod tick;
