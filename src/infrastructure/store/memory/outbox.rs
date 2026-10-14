//! The in-memory notice outbox, part of the shared tables so it is written
//! in the same swap as the deciding change.

use chrono::{DateTime, Utc};

use super::MemoryScheduleStore;
use crate::domain::notify::{
    DrainReason, JournalError, Lease, NoticeOutbox, OutboxNotice, PendingNotices, UndecodableNotice,
};
use crate::domain::schedule::Notice;
use crate::domain::scheduler::StoreError;

#[derive(Clone, Debug)]
struct Row {
    notice: OutboxNotice,
    /// Set by [`MemoryScheduleStore::corrupt_notice`]: the stored payload
    /// no longer decodes (the SQLite store's failure, for parity tests).
    undecodable: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct OutboxTable {
    rows: Vec<Row>,
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
                .any(|row| row.notice.source == source && row.notice.ordinal == ordinal)
            {
                return Err(StoreError::Constraint(format!(
                    "outbox notice {source}#{ordinal} already exists"
                )));
            }
            self.rows.push(Row {
                notice: OutboxNotice {
                    source: source.to_owned(),
                    ordinal,
                    notice: notice.clone(),
                    created_at: super::micros(at),
                    drained_at: None,
                    drained_reason: None,
                },
                undecodable: None,
            });
        }
        Ok(())
    }

    /// The retention purge: drained rows drained before `before`.
    pub(super) fn purge_drained(&mut self, before: DateTime<Utc>) -> u64 {
        let kept = self.rows.len();
        self.rows.retain(|row| {
            row.notice
                .drained_at
                .is_none_or(|drained| drained >= before)
        });
        (kept - self.rows.len()) as u64
    }
}

impl MemoryScheduleStore {
    /// Test support: make a stored notice undecodable, as a damaged SQLite
    /// payload would be.
    pub fn corrupt_notice(&self, source: &str, ordinal: i64, detail: &str) {
        let mut tables = self.tables();
        if let Some(row) = tables
            .outbox
            .rows
            .iter_mut()
            .find(|row| row.notice.source == source && row.notice.ordinal == ordinal)
        {
            row.undecodable = Some(detail.to_owned());
        }
    }
}

impl NoticeOutbox for MemoryScheduleStore {
    async fn pending_notices(&self) -> Result<PendingNotices, JournalError> {
        let tables = self.tables();
        let mut pending = PendingNotices::default();
        for row in tables
            .outbox
            .rows
            .iter()
            .filter(|row| row.notice.drained_at.is_none())
        {
            match &row.undecodable {
                None => pending.notices.push(row.notice.clone()),
                Some(detail) => pending.undecodable.push(UndecodableNotice {
                    source: row.notice.source.clone(),
                    ordinal: row.notice.ordinal,
                    detail: detail.clone(),
                }),
            }
        }
        Ok(pending)
    }

    async fn outbox_notices(&self) -> Result<Vec<OutboxNotice>, JournalError> {
        self.tables()
            .outbox
            .rows
            .iter()
            .map(|row| match &row.undecodable {
                None => Ok(row.notice.clone()),
                Some(detail) => Err(JournalError::Backend(format!(
                    "stored journal row is unreadable: {detail}"
                ))),
            })
            .collect()
    }

    async fn mark_drained(
        &self,
        lease: &Lease,
        source: &str,
        ordinal: i64,
        reason: DrainReason,
        at: DateTime<Utc>,
    ) -> Result<(), JournalError> {
        self.journal_write(|tables| {
            tables.journal.check_live(lease)?;
            let row = tables
                .outbox
                .rows
                .iter_mut()
                .find(|row| row.notice.source == source && row.notice.ordinal == ordinal)
                .ok_or_else(|| {
                    JournalError::StateChanged(format!(
                        "outbox notice {source}#{ordinal} does not exist"
                    ))
                })?;
            if row.notice.drained_at.is_none() {
                row.notice.drained_at = Some(super::micros(at));
                row.notice.drained_reason = Some(reason);
            }
            Ok(())
        })
    }
}
