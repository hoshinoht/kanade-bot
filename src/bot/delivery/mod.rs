//! Delivery: the scheduler tick and the executor that turns planned
//! notifications into Discord posts at most once.
//!
//! Every post is claimed in the delivery journal before transport and bound,
//! resolved or held after it; no database transaction spans a Discord call.

mod alerts;
mod executor;
mod notices;
mod ports;
mod render;
mod tick;

pub use alerts::{ALERT_WINDOW, AdminAlert, AlertRecorder, AlertSink, AlertThrottle};
pub use executor::{Executor, Replacement, SendFailure, SendOutcome, SendReport};
pub use notices::{NoticeReport, NoticeSend};
pub use ports::{FixedClock, IdsRef, StoreRef};
pub use render::render;
pub use tick::{
    DEFAULT_MAX_SENDS_PER_TICK, Delivery, DeliveryConfig, DeliveryError, DigestOutcome,
    DigestReport, DispatchReport, TICK_OPERATION, TickReport,
};
