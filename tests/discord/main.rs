//! Discord adapter tests. All offline: fakes, Twilight models built from
//! JSON, and a loopback HTTP stub; nothing contacts Discord.

mod cards;
mod commands;
mod fake_transport;
mod gateway;
mod guild;
mod guild_cache;
mod http_transport;
mod members;
mod mentions;
mod messages;
mod reactions;
mod replies;
mod slash;
mod support;
