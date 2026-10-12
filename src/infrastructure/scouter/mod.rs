//! MapleScouter estimates for module `character_data` (admin only, on
//! demand, one lookup at a time): the client of the private scouter sidecar
//! (`KANADE_SCOUTER_URL`, a separate Compose project with no
//! authentication), the lookup jobs and their in-memory 24-hour cache, and
//! the one table mapping scouter boss keys to catalog tokens.
//!
//! The sidecar is only ever sent roster IGNs ([`RosterIgn`], built from a
//! roster `Character`) as `ign` or `igns`: never a preset, never a Nexon
//! field. Results never reach a model, a file or a log line (logs carry
//! codes and counts only), and are labelled "Estimate for planning only,
//! not eligibility".

mod client;
mod estimates;
mod ign;
mod keys;
mod wire;

pub use client::{LookupError, MAX_BATCH, ScouterClient, TIMEOUT, query};
pub use estimates::{
    CACHE_TTL, Estimate, Estimates, Failure, FailureKind, Found, Job, JobRequest, JobState,
    MAX_CACHED, MAX_JOBS, Missing, MissingReason, ScouterHealth, ScouterState, Start, Subject,
    Target,
};
pub use ign::RosterIgn;
pub use keys::{BOSS_KEYS, catalog_token, scouter_keys};
pub use wire::{Card, Combined, Member};
