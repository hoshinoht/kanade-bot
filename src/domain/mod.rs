//! Pure scheduling domain ported from v4 `bot/domain`: no I/O and no clock reads.
//!
//! Error `Display` output reproduces the v4 messages byte-for-byte because they
//! reach members verbatim.

#[macro_use]
mod forward;
pub(crate) use forward::forward_boxed;
#[macro_use]
mod spelled;
pub(crate) use spelled::spelled;

pub mod attendance;
pub mod catalog;
pub mod characters;
pub mod chardata;
pub mod completion;
pub mod drafts;
pub mod history;
pub mod ids;
pub mod members;
pub mod model_log;
pub mod notify;
pub mod ownership;
pub mod proposals;
pub(crate) mod pytext;
pub mod requests;
pub mod schedule;
pub mod scheduler;
pub mod settings;
pub mod time;
pub mod weeks;
