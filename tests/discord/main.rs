//! Discord adapter tests. All offline: fakes, Twilight models built from
//! JSON, and a loopback HTTP stub; nothing contacts Discord.

mod commands;
mod fake_transport;
mod gateway;
mod http_transport;
mod members;
mod mentions;
mod reactions;
mod support;
