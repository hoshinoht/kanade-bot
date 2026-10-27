//! The wall clock every surface reads, injectable for tests.

use std::sync::Arc;

use chrono::{DateTime, Utc};

pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

/// Wall clock without chrono's `clock` feature; before the epoch reads as the epoch.
pub fn system_now() -> DateTime<Utc> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    DateTime::from_timestamp(
        i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        since.subsec_nanos(),
    )
    .unwrap_or(DateTime::UNIX_EPOCH)
}
