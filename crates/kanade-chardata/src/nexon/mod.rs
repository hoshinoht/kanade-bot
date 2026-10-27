//! The NEXON Open API (MapleSEA) client for module `character_data`: base
//! `https://open.api.nexon.com`, header `x-nxopen-api-key`, the `id`,
//! `character/basic` and `character/stat` endpoints, and the process-wide
//! limiter. Error codes map to backoffs in `domain::chardata::Failure`.

mod client;
mod limiter;
mod wire;

pub use client::{ApiKey, CallError, HOST_NAME, KEY_HEADER, NexonClient};
pub use limiter::{Limiter, SPACING};
pub use wire::{Basic, COMBAT_POWER_STAT};
