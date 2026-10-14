//! In-memory `AuthAuditStore`, mirroring SQLite's `auth_audit`: rows keep
//! insertion order, an append first drops a batch past retention.

use crate::domain::scheduler::StoreError;
use crate::infrastructure::store::auth_audit::{
    AUDIT_PRUNE_BATCH, AUDIT_RETENTION, AuditFilter, AuditRow, AuthAuditStore,
};

#[derive(Debug, Default)]
pub(super) struct AuditTable {
    rows: Vec<AuditRow>,
    next_seq: i64,
}

impl super::MemoryScheduleStore {
    fn audit(&self) -> std::sync::MutexGuard<'_, AuditTable> {
        self.audit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn matches(row: &AuditRow, filter: &AuditFilter) -> bool {
    filter.realm.is_none_or(|realm| row.realm == realm)
        && filter.event.is_none_or(|event| row.event == event)
        && filter
            .actor
            .as_deref()
            .is_none_or(|actor| row.actor.as_deref() == Some(actor))
        && filter.from.is_none_or(|from| row.at >= from)
        && filter.to.is_none_or(|to| row.at < to)
        && filter.before_seq.is_none_or(|before| row.seq < before)
}

impl AuthAuditStore for super::MemoryScheduleStore {
    async fn append_audit(&self, mut row: AuditRow) -> Result<i64, StoreError> {
        row.check()?;
        let mut table = self.audit();
        if let Some(cutoff) = row.at.checked_sub_signed(AUDIT_RETENTION) {
            let mut budget = AUDIT_PRUNE_BATCH;
            table.rows.retain(|kept| {
                let drop = budget > 0 && kept.at < cutoff;
                if drop {
                    budget -= 1;
                }
                !drop
            });
        }
        table.next_seq += 1;
        row.seq = table.next_seq;
        table.rows.push(row);
        Ok(table.next_seq)
    }

    async fn audit_page(&self, filter: &AuditFilter) -> Result<Vec<AuditRow>, StoreError> {
        Ok(self
            .audit()
            .rows
            .iter()
            .rev()
            .filter(|row| matches(row, filter))
            .take(filter.page_size() as usize)
            .cloned()
            .collect())
    }
}
