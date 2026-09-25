//! The gateway connection: intents, the single shard and its event loop.

mod close;
mod intents;
mod runner;
mod status;

use twilight_gateway::{Shard, ShardId};

pub use close::CloseReason;
pub use intents::{INTENTS, WANTED_EVENTS};
pub use runner::{EventSource, GatewayError, Live, RunExit, RunnerConfig, run, run_live};
pub use status::{Connection, ConnectionStatus};

/// The one shard a single-guild bot needs. Construction does not connect;
/// the first poll does. The process Rustls provider must be installed first.
pub fn shard(token: String) -> Shard {
    Shard::new(ShardId::ONE, token, INTENTS)
}
