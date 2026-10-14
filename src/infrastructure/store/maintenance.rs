//! Store upkeep serve runs off the request path, after each daily log
//! retention run (user decision 2026-10-11).

use crate::domain::scheduler::StoreError;

/// What one `PRAGMA wal_checkpoint(TRUNCATE)` reported: `busy` when a
/// reader or writer kept it from finishing, and the WAL's frame counts
/// (`-1` when the database is not in WAL mode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WalCheckpoint {
    pub busy: bool,
    pub log: i64,
    pub checkpointed: i64,
}

pub trait StoreMaintenance {
    /// Copy the WAL back into the database and truncate it, on the write
    /// connection, which is held for the checkpoint alone. Readers in
    /// progress may keep it from finishing (`busy`); that is not an error.
    fn checkpoint_wal(&self) -> impl Future<Output = Result<WalCheckpoint, StoreError>> + Send;
}
