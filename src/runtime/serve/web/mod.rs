//! The `web` component's wiring: the admin and public API state, both
//! sign-in realms, session pruning, the secrets Setup saves and, without
//! the bot, the guild read over REST.

pub mod api;
pub mod rest;
pub(super) mod sessions;
pub(super) mod setup_secrets;
