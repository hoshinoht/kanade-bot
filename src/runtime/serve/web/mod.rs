//! The `web` component's wiring: the admin and public API state, both
//! sign-in realms, session pruning and the secrets Setup saves.

pub mod api;
pub(super) mod sessions;
pub(super) mod setup_secrets;
