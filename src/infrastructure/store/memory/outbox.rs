//! The in-memory notice outbox, part of the shared tables so it is written
//! in the same swap as the deciding change.

use chrono::{DateTime, Utc};

use super::MemoryScheduleStore;
use crate::domain::notify::{JournalError, Lease, NoticeOutbox, OutboxNotice};
use crate::domain::schedule::Notice;
use crate::domain::scheduler::StoreError;

#[derive(Clone, Debug, Default)]
pub(super) struct OutboxTable {
    rows: Vec<OutboxNotice>,
}

impl OutboxTable {
    /// Write `notices` as `(source, 0..)`; a taken key refuses the write.
    pub(super) fn enqueue(
        &mut self,
        source: &str,
        notices: &[Notice],
        at: DateTime<Utc>,
    ) -> Result<(), StoreError> {
        for (ordinal, notice) in notices.iter().enumerate() {
            let ordinal = i64::try_from(ordinal).unwrap_or(i64::MAX);
            if self
                .rows
                .iter()
                .any(|row| row.source == source && row.ordinal == ordinal)
            {
                return Err(StoreError::Constraint(format!(
                    "outbox notice {source}#{ordinal} already exists"
                )));
            }
            self.rows.push(OutboxNotice {
                source: source.to_owned(),
                ordinal,
                notice: notice.clone(),
                created_at: super::micros(at),
                drained_at: None,
            });
        }
        Ok(())
    }
}

impl NoticeOutbox for MemoryScheduleStore {
    async fn pending_notices(&self) -> Result<Vec<OutboxNotice>, JournalError> {
        let tables = self.tables();
        Ok(tables
            .outbox
            .rows
            .iter()
            .filter(|row| row.drained_at.is_none())
            .cloned()
            .collect())
    }

    async fn outbox_notices(&self) -> Result<Vec<OutboxNotice>, JournalError> {
        Ok(self.tables().outbox.rows.clone())
    }

    async fn mark_drained(
        &self,
        lease: &Lease,
        source: &str,
        ordinal: i64,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            let row = tables
                .outbox
                .rows
                .iter_mut()
                .find(|row| row.source == source && row.ordinal == ordinal)
                .ok_or_else(|| {
                    JournalError::StateChanged(format!(
                        "outbox notice {source}#{ordinal} does not exist"
                    ))
                })?;
            row.drained_at = row.drained_at.or(Some(super::micros(at)));
            Ok(())
        })
    }
}
