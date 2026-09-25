//! The single write connection, handed out one transaction at a time.
//!
//! A [`WriterLease`] takes the connection out of its slot. Finishing it
//! healthy puts it back; an unhealthy finish or a dropped lease (a cancelled
//! commit) orphans it instead: a task closes it, letting SQLite finish or roll
//! back whatever was in flight, and [`Writer::shutdown`] awaits those tasks.
//! The next lease connects afresh, so no transaction state is ever inherited.
//! Every lease first checks that the configured path still names the owned
//! database inode, so no write lands in a moved or replaced file.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use sqlx::{Connection, SqliteConnection};
use tokio::sync::{Mutex, MutexGuard};
use tokio::task::JoinHandle;

use super::owner::DbIdentity;
use super::{SqliteStoreError, connect};

/// Why no write connection was handed out.
#[derive(Debug)]
pub(crate) enum LeaseError {
    Identity(SqliteStoreError),
    Connect(sqlx::Error),
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity(error) => error.fmt(f),
            Self::Connect(error) => error.fmt(f),
        }
    }
}

pub(super) struct Writer {
    db_path: PathBuf,
    identity: Arc<DbIdentity>,
    slot: Mutex<Option<SqliteConnection>>,
    orphans: std::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl Writer {
    pub(super) fn new(db_path: PathBuf, identity: Arc<DbIdentity>, conn: SqliteConnection) -> Self {
        Self {
            db_path,
            identity,
            slot: Mutex::new(Some(conn)),
            orphans: std::sync::Mutex::default(),
        }
    }

    /// Wait for the connection, reconnecting if the last one was orphaned.
    pub(super) async fn lease(&self) -> Result<WriterLease<'_>, LeaseError> {
        let mut slot = self.slot.lock().await;
        self.identity.verify().map_err(LeaseError::Identity)?;
        let conn = match slot.take() {
            Some(conn) => conn,
            None => {
                let conn = connect::writer(&self.db_path)
                    .await
                    .map_err(LeaseError::Connect)?;
                // The path may have been swapped while it was being opened.
                if let Err(error) = self.identity.verify() {
                    let _ = conn.close().await;
                    return Err(LeaseError::Identity(error));
                }
                conn
            }
        };
        Ok(WriterLease {
            slot,
            conn: Some(conn),
            writer: self,
        })
    }

    fn orphan(&self, conn: SqliteConnection) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            // Outside a runtime the drop closes it on the worker thread.
            return;
        };
        let task = runtime.spawn(async move {
            let _ = conn.close().await;
        });
        let mut orphans = self
            .orphans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        orphans.retain(|task| !task.is_finished());
        orphans.push(task);
    }

    /// Close the connection and wait for every orphaned one to close.
    pub(super) async fn shutdown(self) -> Result<(), sqlx::Error> {
        let closed = match self.slot.into_inner() {
            Some(conn) => conn.close().await,
            None => Ok(()),
        };
        let orphans = std::mem::take(
            &mut *self
                .orphans
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for task in orphans {
            let _ = task.await;
        }
        closed
    }
}

/// Exclusive use of the write connection for one transaction.
pub(crate) struct WriterLease<'a> {
    slot: MutexGuard<'a, Option<SqliteConnection>>,
    conn: Option<SqliteConnection>,
    writer: &'a Writer,
}

impl WriterLease<'_> {
    pub(crate) fn conn(&mut self) -> &mut SqliteConnection {
        self.conn
            .as_mut()
            .unwrap_or_else(|| unreachable!("a lease holds its connection until finished"))
    }

    /// Return the connection, or orphan it when its state is unknown.
    pub(crate) fn finish(mut self, healthy: bool) {
        if let Some(conn) = self.conn.take() {
            if healthy {
                *self.slot = Some(conn);
            } else {
                self.writer.orphan(conn);
            }
        }
    }
}

impl Drop for WriterLease<'_> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.writer.orphan(conn);
        }
    }
}
