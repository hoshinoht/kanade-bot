//! What extraction and chat need persisted: the watched-message cache, the
//! extraction and chat logs the admin Extractions/Chat pages filter
//! (`docs/v5/admin-api.md`), rescan jobs, per-member chat allowance
//! overrides and the one-tip-per-boss-week self-service record. Types and
//! the store port only; nothing here does I/O.

mod filter;
mod outcome;
mod port;
mod records;
mod retention;

pub use filter::{
    ChatFilter, ExtractionFilter, LogCursor, LogFacets, LogPage, MAX_PAGE, page_size,
};
pub use outcome::{ChatOutcome, ExtractionOutcome, RescanStatus};
pub use port::{MessageUpsert, ModelLogStore};
pub use records::{
    AllowanceOverride, ChatInteraction, ChatRound, ExtractionLog, RescanJob, WatchedMessage,
};
pub use retention::{DEFAULT_LOG_RETENTION, PRUNE_BATCH, PruneCounts, retention_cutoff};
