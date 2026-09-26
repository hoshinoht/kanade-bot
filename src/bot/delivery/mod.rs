//! Delivery: the scheduler tick and the executor that turns planned
//! notifications into Discord posts at most once.
//!
//! Every post is claimed in the delivery journal before transport and bound,
//! resolved or held after it; no database transaction spans a Discord call.

mod alerts;
mod card_records;
pub mod cards;
mod executor;
mod notice_text;
mod notices;
mod ports;
mod refresh;
mod render;
mod tick;

pub use alerts::{ALERT_WINDOW, AdminAlert, AlertRecorder, AlertSink, AlertThrottle, LogAlerts};
pub use executor::{Executor, Replacement, SendFailure, SendOutcome, SendReport};
pub use notice_text::render_notice;
pub use notices::{NoticeReport, NoticeSend};
pub use ports::{FixedClock, IdsRef, StoreRef};
pub use refresh::{CardRefresh, Now};
pub use render::render;
pub use tick::{
    DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, DeliveryError, DigestOutcome,
    DigestReport, DispatchReport, TICK_OPERATION, TickReport,
};
