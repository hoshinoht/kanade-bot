//! Delivery: the scheduler tick and the executor that turns planned
//! notifications into Discord posts at most once.
//!
//! Every post is claimed in the delivery journal before transport and bound,
//! resolved or held after it; no database transaction spans a Discord call.

mod alerts;
mod card_records;
pub mod cards;
pub mod debug;
mod declines;
mod executor;
mod notice_text;
mod notices;
mod ports;
mod pregen;
pub mod preview;
mod refresh;
mod render;
mod tick;

pub use alerts::{ALERT_WINDOW, AdminAlert, AlertRecorder, AlertSink, AlertThrottle, LogAlerts};
pub use debug::{
    DebugCardStore, DebugDesk, MAX_HEADER_TRIES, PostedDebugCard, SAMPLE_RUN_ID, TEST_PREFIX,
};
pub use declines::DeclineReport;
pub use executor::{Executor, Replacement, SendFailure, SendOutcome, SendReport};
pub use notice_text::render_notice;
pub use notices::{NoticeReport, NoticeSend};
pub use ports::{FixedClock, IdsRef, StoreRef};
pub use pregen::{
    HEADER_HORIZON, HeaderPregen, HeaderTime, MAX_ATTEMPTS_PER_KEY, MAX_BUSY_RETRIES,
    MAX_CATCHUP_PER_TICK, PREGEN_DEADLINE, PREGEN_INTERVAL, PregenReport, SEEN_HORIZON,
};
pub use refresh::{CardRefresh, MAX_PENDING_RUNS, Now, RefreshQueue};
pub use render::render;
pub use tick::{
    DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, DeliveryError, DigestOutcome,
    DigestReport, DispatchReport, TICK_OPERATION, TickReport,
};
