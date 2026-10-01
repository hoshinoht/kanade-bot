//! Extraction orchestration (v4 `bot/extract/pipeline.py`): watched messages
//! are cached, gated and buffered per channel, and each burst becomes one
//! governed model call per prompt-sized piece, planned changes, proposals
//! through the scheduler and one extraction log row per call. Discord is
//! reached only through the ports (`bot::cards::CardOutbox` posts the cards).
//! `self_service` applies the self-service redirect and the weekly nudge.

mod call;
mod commit;
mod config;
mod debounce;
mod driver;
mod extractor;
mod outcome;
mod ports;
mod refusal;
mod self_service;

pub(crate) use call::CallRecord;
pub use call::Failure;
pub use config::{
    CONTEXT_WINDOW, CallContext, DEFAULT_BACKLOG_CAPACITY, DEFAULT_CALL_TIMEOUT,
    DEFAULT_CONTEXT_MESSAGES, DEFAULT_CONTEXT_TOKENS, DEFAULT_DEBOUNCE, DEFAULT_DRAIN_INTERVAL,
    DEFAULT_MIN_CONFIDENCE, DEFAULT_PERMIT_WAIT, LiveContext, PipelineConfig, RECENT_SCHEDULING,
    SelfServiceConfig, UnpublishedEffort, check_reasoning_effort,
};
pub use debounce::Bursts;
pub use driver::Pipeline;
pub use extractor::{
    CALL_CANCELLED, CALL_SWITCHED_OFF, Deps, Extractor, HISTORY_UNREADABLE, PassReport,
    SCHEDULE_UNREADABLE,
};
pub use outcome::extraction_outcome;
pub use ports::{
    AuthorKind, BacklogDrop, Card, CardEntry, ChatAnswer, Guild, IncomingMessage, MessageEvent,
    MessageOrigin, Outbox, Personas, PostResult, Proposer, Redirected, SelfServiceDeps,
    SelfServiceTip,
};
pub use refusal::{refusal, refusal_code};
