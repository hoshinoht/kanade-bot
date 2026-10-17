//! Scouter estimates for module `character_data` (admin only, queued and
//! looked up one at a time): the client of the private scouter sidecar
//! (`KANADE_SCOUTER_URL`, a separate Compose project with no
//! authentication), the lookup jobs and their results (kept up to 30 days
//! in memory and in the character-data file, never in a backup), the party
//! resolution "Look up party" and the background refresh share, the
//! refresh's choice of upcoming parties to keep fresh, and the one table
//! mapping scouter boss keys to catalog tokens.
//!
//! The sidecar is only ever sent roster IGNs ([`RosterIgn`], built from a
//! roster `Character`) as `ign` or `igns`: never a preset, never a Nexon
//! field. Results never reach a model, a log or any file but the
//! character-data file (logs carry codes and counts only), and are
//! labelled "For planning only, not eligibility".

mod client;
mod estimates;
mod ign;
mod keys;
mod party;
pub mod refresh;
mod saved;
mod wire;

pub use client::{LookupError, MAX_BATCH, ScouterClient, TIMEOUT, query};
pub use estimates::{
    BACKGROUND_CAP, BACKGROUND_GAP, BACKOFF, CACHE_TTL, CAP_WINDOW, Estimates, Failure,
    FailureKind, Found, FreshDays, JOB_TTL, Job, JobRequest, JobSource, JobState, MAX_CACHED,
    MAX_JOBS, MAX_QUEUED, MemberOutcome, Missing, MissingReason, REFRESHER, ScouterHealth,
    ScouterState, Start, Subject, Target, Totals,
};
pub use ign::RosterIgn;
pub use keys::{BOSS_KEYS, catalog_token, scouter_keys};
pub use party::{BossParty, party_subjects, push_subject};
pub use refresh::{Upcoming, UpcomingFuture, UpcomingParties, UpcomingRun};
pub use saved::{CarrierOwner, SavedParty, saved_party};
pub use wire::{Card, Combined, Member};
