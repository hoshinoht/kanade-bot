use super::MemoryScheduleStore;
use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::maintenance::{StoreMaintenance, WalCheckpoint};

/// No WAL in memory: nothing to checkpoint.
impl StoreMaintenance for MemoryScheduleStore {
    async fn checkpoint_wal(&self) -> Result<WalCheckpoint, StoreError> {
        Ok(WalCheckpoint::default())
    }
}
