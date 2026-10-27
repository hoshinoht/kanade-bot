//! Test support: injected store failures and journal shortcuts. Each fault
//! fails before anything is written.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use sqlx::Connection;

use super::schedule::store_error;
use super::{SqliteCharData, SqliteStore, journal};
use kanade_domain::notify::{AttemptId, JournalError};
use kanade_domain::scheduler::StoreError;

/// The armed faults of one [`SqliteStore`].
#[derive(Default)]
pub(super) struct Faults {
    /// Commits still allowed before one fails, if armed.
    commit_after: Mutex<Option<usize>>,
    card_record_reads: AtomicUsize,
    card_record_writes: AtomicUsize,
    digest_phrase_reads: AtomicUsize,
    digest_phrase_writes: AtomicUsize,
    exact_marks: AtomicBool,
}

/// The armed faults of one [`SqliteCharData`].
#[derive(Default)]
pub(super) struct CharDataFaults {
    unlinks: AtomicBool,
    put_links: AtomicBool,
}

/// Take one armed failure from `count`.
fn take(count: &AtomicUsize) -> bool {
    count
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
        .is_ok()
}

fn injected(armed: bool, what: &str) -> Result<(), StoreError> {
    if armed {
        Err(StoreError::Backend(format!("injected {what} failure")))
    } else {
        Ok(())
    }
}

impl Faults {
    /// Every schedule commit (`commit`, `commit_with_decline_notices`),
    /// including replays, consumes the configured failure count.
    pub(super) fn commit(&self) -> Result<(), StoreError> {
        let mut after = self
            .commit_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let fail = match *after {
            Some(0) => {
                *after = None;
                true
            }
            Some(remaining) => {
                *after = Some(remaining - 1);
                false
            }
            None => false,
        };
        injected(fail, "schedule commit")
    }

    pub(super) fn card_record_read(&self) -> Result<(), StoreError> {
        injected(take(&self.card_record_reads), "reminder card record read")
    }

    pub(super) fn card_record_write(&self) -> Result<(), StoreError> {
        injected(take(&self.card_record_writes), "reminder card record write")
    }

    pub(super) fn digest_phrase_read(&self) -> Result<(), StoreError> {
        injected(take(&self.digest_phrase_reads), "digest phrase read")
    }

    pub(super) fn digest_phrase_write(&self) -> Result<(), StoreError> {
        injected(take(&self.digest_phrase_writes), "digest phrase write")
    }

    pub(super) fn exact_marks(&self) -> Result<(), StoreError> {
        if self.exact_marks.load(Ordering::SeqCst) {
            Err(StoreError::Backend("messages are unwritable".into()))
        } else {
            Ok(())
        }
    }
}

impl CharDataFaults {
    pub(super) fn unlink(&self) -> Result<(), StoreError> {
        if self.unlinks.load(Ordering::SeqCst) {
            Err(StoreError::Backend("unlink failed (test hook)".into()))
        } else {
            Ok(())
        }
    }

    pub(super) fn put_link(&self) -> Result<(), StoreError> {
        if self.put_links.load(Ordering::SeqCst) {
            Err(StoreError::Backend("put_link failed (test hook)".into()))
        } else {
            Ok(())
        }
    }
}

impl SqliteStore {
    /// Test hook: after `successful_commits` more schedule commits, the
    /// next one fails as a backend error (once).
    pub fn fail_commit_after(&self, successful_commits: usize) {
        *self
            .faults
            .commit_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(successful_commits);
    }

    /// Test hook: the next `card_record` fails; calls add up.
    pub fn fail_next_card_record_read(&self) {
        self.faults.card_record_reads.fetch_add(1, Ordering::SeqCst);
    }

    /// Test hook: the next `save_card_record` fails; calls add up.
    pub fn fail_next_card_record_write(&self) {
        self.faults
            .card_record_writes
            .fetch_add(1, Ordering::SeqCst);
    }

    /// Test hook: the next `digest_phrase` fails; calls add up.
    pub fn fail_next_digest_phrase_read(&self) {
        self.faults
            .digest_phrase_reads
            .fetch_add(1, Ordering::SeqCst);
    }

    /// Test hook: the next `save_digest_phrase` fails; calls add up.
    pub fn fail_next_digest_phrase_write(&self) {
        self.faults
            .digest_phrase_writes
            .fetch_add(1, Ordering::SeqCst);
    }

    /// Test hook: every `mark_read_exact` fails as a backend error while
    /// `fail` holds.
    pub fn fail_exact_marks(&self, fail: bool) {
        self.faults.exact_marks.store(fail, Ordering::SeqCst);
    }

    /// Test support for a grouped card: maps `run_id` onto the card already
    /// posted as `message_id` (a bound delivery attempt), as a binding for
    /// several runs would. Mapping it twice is a no-op.
    ///
    /// # Panics
    /// No bound attempt posted `message_id`, or the write failed.
    pub async fn test_map_card_run(&self, message_id: &str, run_id: &str) {
        assert!(
            self.map_card_run(message_id, run_id)
                .await
                .expect("card run is mapped"),
            "no bound delivery attempt posted message {message_id}"
        );
    }

    /// `false` when no bound attempt posted `message_id`.
    async fn map_card_run(&self, message_id: &str, run_id: &str) -> Result<bool, StoreError> {
        write_txn!(self, tx, async {
            let found = sqlx::query(
                "SELECT 1 FROM delivery_attempts WHERE message_id = ?1 AND state = 'bound'",
            )
            .bind(message_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(store_error)?
            .is_some();
            if found {
                sqlx::query(
                    "INSERT INTO delivery_card_runs (attempt_id, channel_id, message_id, run_id) \
                     SELECT attempt_id, channel_id, message_id, ?2 FROM delivery_attempts \
                     WHERE message_id = ?1 AND state = 'bound' ORDER BY attempt_id LIMIT 1 \
                     ON CONFLICT (attempt_id, run_id) DO NOTHING",
                )
                .bind(message_id)
                .bind(run_id)
                .execute(&mut *tx)
                .await
                .map_err(store_error)?;
            }
            Ok(found)
        })
    }

    /// Test support: release every target of the bound `attempt` and retire
    /// it (actor `test`, reason `test retirement`), as a confirmed deletion
    /// would.
    ///
    /// # Panics
    /// The attempt is not live, or the write failed.
    pub async fn test_retire_bound_attempt(&self, attempt: &AttemptId) {
        self.retire_bound_attempt(attempt)
            .await
            .expect("attempt is retired");
    }

    async fn retire_bound_attempt(&self, attempt: &AttemptId) -> Result<(), JournalError> {
        // Test retirement needs only a timestamp for the recorded transition.
        let now = super::backup::now_seconds();
        write_txn!(JournalError, journal::backend; self, tx,
            journal::release_and_retire(&mut tx, attempt, "test", "test retirement", now)
        )
    }
}

impl SqliteCharData {
    /// Test hook: every `unlink` fails (nothing written) while `on`.
    pub fn fail_unlinks(&self, on: bool) {
        self.faults.unlinks.store(on, Ordering::SeqCst);
    }

    /// Test hook: every `put_link` fails (nothing written) while `on`.
    pub fn fail_put_links(&self, on: bool) {
        self.faults.put_links.store(on, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};

    use super::*;
    use crate::chardata_conformance::active;
    use crate::conformance::meta;
    use kanade_domain::chardata::CharDataStore;
    use kanade_domain::model_log::ModelLogStore;
    use kanade_domain::notify::{
        CardRecord, DeclineNoticeStore, DedupeKey, DeliveryTarget, DigestPhraseStore,
        ReminderCardStore,
    };
    use kanade_domain::schedule::ChangeSet;
    use kanade_domain::scheduler::ScheduleStore;

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 31, 8, 0, 0)
            .single()
            .expect("instant")
    }

    fn injected<T: std::fmt::Debug>(result: Result<T, StoreError>, what: &str) {
        match result {
            Err(StoreError::Backend(message)) => assert_eq!(message, what),
            other => panic!("expected {what:?}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_commit_fails_once_after_the_allowed_commits() {
        let store = SqliteStore::open_temp().await;
        let commit = || store.commit(0, ChangeSet::default(), meta());
        commit().await.expect("not armed");
        store.fail_commit_after(1);
        commit().await.expect("the allowed commit");
        // Decline-notice commits share the path and the count.
        injected(
            store
                .commit_with_decline_notices(0, ChangeSet::default(), meta(), vec![], vec![])
                .await,
            "injected schedule commit failure",
        );
        commit().await.expect("fired once");
        store.close().await.expect("close");
    }

    #[tokio::test]
    async fn card_record_and_digest_phrase_faults_fire_once_per_call() {
        let store = SqliteStore::open_temp().await;
        let card = DedupeKey::native(&[DeliveryTarget::Reminder("m-1".into())]).expect("key");
        let card = card.as_str();
        let record = CardRecord {
            kind: "day_of".into(),
            heading: None,
            slots: None,
        };
        store.fail_next_card_record_read();
        store.fail_next_card_record_read();
        for _ in 0..2 {
            injected(
                store.card_record(card).await,
                "injected reminder card record read failure",
            );
        }
        assert_eq!(store.card_record(card).await.expect("read"), None);
        store.fail_next_card_record_write();
        injected(
            store.save_card_record(card, &record, at()).await,
            "injected reminder card record write failure",
        );
        assert_eq!(
            store.card_record(card).await.expect("read"),
            None,
            "nothing written"
        );
        store
            .save_card_record(card, &record, at())
            .await
            .expect("save");

        let digest = DedupeKey::native(&[DeliveryTarget::Digest(at())]).expect("key");
        let digest = digest.as_str();
        store.fail_next_digest_phrase_read();
        injected(
            store.digest_phrase(digest).await,
            "injected digest phrase read failure",
        );
        assert_eq!(store.digest_phrase(digest).await.expect("read"), None);
        store.fail_next_digest_phrase_write();
        injected(
            store.save_digest_phrase(digest, "Let's go!", at()).await,
            "injected digest phrase write failure",
        );
        assert_eq!(
            store.digest_phrase(digest).await.expect("read"),
            None,
            "nothing written"
        );
        assert_eq!(
            store
                .save_digest_phrase(digest, "Let's go!", at())
                .await
                .expect("save"),
            "Let's go!"
        );
        store.close().await.expect("close");
    }

    #[tokio::test]
    async fn exact_marks_fail_while_armed() {
        let store = SqliteStore::open_temp().await;
        store.fail_exact_marks(true);
        for _ in 0..2 {
            injected(
                store.mark_read_exact(&[], at()).await,
                "messages are unwritable",
            );
        }
        store.fail_exact_marks(false);
        store.mark_read_exact(&[], at()).await.expect("disarmed");
        store.close().await.expect("close");
    }

    #[tokio::test]
    async fn chardata_unlinks_and_link_puts_fail_while_armed() {
        let store = SqliteCharData::open_temp().await;
        store.fail_put_links(true);
        injected(
            store.put_link(active("c1"), 0).await,
            "put_link failed (test hook)",
        );
        assert_eq!(store.links().await.expect("links"), [], "nothing written");
        store.fail_put_links(false);
        store.put_link(active("c1"), 0).await.expect("put");
        store.fail_unlinks(true);
        for _ in 0..2 {
            injected(
                store.unlink("c1".into(), at()).await,
                "unlink failed (test hook)",
            );
        }
        assert_eq!(store.links().await.expect("links").len(), 1, "still linked");
        store.fail_unlinks(false);
        assert!(
            store
                .unlink("c1".into(), at())
                .await
                .expect("unlink")
                .is_some()
        );
        store.close().await.expect("close");
    }
}
