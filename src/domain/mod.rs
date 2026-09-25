//! Pure scheduling domain ported from v4 `bot/domain`: no I/O and no clock reads.
//!
//! Error `Display` output reproduces the v4 messages byte-for-byte because they
//! reach members verbatim and are frozen in `docs/v5/vectors/domain`.

pub mod attendance;
pub mod catalog;
pub mod drafts;
pub mod history;
pub mod ids;
pub mod members;
pub mod model_log;
pub mod notify;
pub(crate) mod pytext;
pub mod requests;
pub mod schedule;
pub mod scheduler;
pub mod time;
pub mod weeks;
