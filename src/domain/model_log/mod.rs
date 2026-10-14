//! What extraction and chat need persisted: the watched-message cache, the
//! extraction and chat logs the admin Extractions/Chat pages filter
//! (`docs/notes/admin-api.md`), rescan jobs, per-member chat allowance
//! overrides, the one-tip-per-boss-week self-service record and the
//! Rewrites log. Types and the store ports only; nothing here does I/O.

mod filter;
mod masked;
mod outcome;
mod port;
mod reasoning;
mod records;
mod retention;
mod rewrite;

pub use filter::{
    ChatFilter, ExtractionFilter, LogCursor, LogFacets, LogPage, MAX_PAGE, page_size,
};
pub use masked::{MaskedName, MaskedRound, MaskedTurn};
pub use outcome::{ChatOutcome, ExtractionOutcome, RescanStatus};
pub use port::{MessageUpsert, ModelLogStore, ReadMessage};
pub use reasoning::{REASONING_CAP, REASONING_TRUNCATED, capped_reasoning};
pub use records::{
    AllowanceOverride, ChatInteraction, ChatRound, ExtractionLog, ExtractionRefusal,
    PROFILE_SOURCES, ROUTES, RescanJob, WatchedMessage, in_order, is_correlation_id,
};
pub use retention::{DEFAULT_LOG_RETENTION, PRUNE_BATCH, PruneCounts, retention_cutoff};
pub use rewrite::{
    CODE_CAP, CONTEXT_CAP, LINE_CAP, PROMPT_CAP, PROMPT_TRUNCATED, REPLY_CAP, REPLY_TRUNCATED,
    REWRITE_VERDICTS, RewriteFacets, RewriteFilter, RewriteKind, RewriteLog, RewriteLogStore,
    RewriteStage, capped_prompt, capped_reply,
};
