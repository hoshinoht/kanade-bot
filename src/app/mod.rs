//! Services the Discord bot and the web server share, with neither's
//! transport: the scheduler writer and its rules, the object-safe store
//! reads, the guild facts and staff rule, the live settings desk, rescans,
//! roster persistence and the ports one side hands the other.

pub mod access;
pub mod clock;
pub mod config;
pub mod crypto;
pub mod guild;
pub mod ids;
mod model_catalog;
pub mod ownership;
pub mod ports;
pub mod read;
pub mod rescan;
pub mod roster;
pub mod write;
