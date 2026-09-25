//! Change-history integrity: the append-only triggers, tamper detection on
//! both stores, and chain continuity across restarts.

use std::collections::BTreeSet;

use kanade::domain::history::{
    ChangeFilter, ChangeHistory, ChangeQuery, CheckedChange, HistoryRefusal, RevertMode,
    RevertOutcome, RowKey,
};
use kanade::domain::schedule::{Change, ReminderPolicy};
use kanade::domain::scheduler::{ScheduleStore, SchedulerError, Scope};
use kanade::infrastructure::store::{
    BackupManifest, MemoryScheduleStore, SqliteStore, SqliteStoreConfig, SqliteStoreError,
};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

use crate::support::{TempDir, commit, fixed, seed, tamper};

async fn verify<S: ChangeHistory>(store: &S) -> kanade::domain::history::HistoryVerification {
    store.verify_history().await.expect("verify")
}

/// Seed plus `extra` single-row commits.
async fn history<S: ScheduleStore>(store: &S, extra: usize) {
    seed(store).await;
    for n in 0..extra {
        commit(store, vec![Change::PutFixedRun(fixed(&format!("f-{n}")))]).await;
    }
}

async fn raw(config: &SqliteStoreConfig, sql: &str) -> Result<(), sqlx::Error> {
    let mut conn = SqliteConnectOptions::new()
        .filename(&config.db_path)
        .connect()
        .await?;
    let result = sqlx::raw_sql(sql).execute(&mut conn).await.map(|_| ());
    conn.close().await?;
    result
}

#[tokio::test]
async fn memory_tampering_is_detected() {
    let store = MemoryScheduleStore::new();
    history(&store, 3).await;
    assert!(verify(&store).await.is_intact());
    store.tamper_change(2, |record| {
        record.origin.actor = kanade::domain::history::Actor::member("impostor");
    });
    let broken = verify(&store).await.first_broken.expect("detected");
    assert_eq!(broken.seq, 2, "{}", broken.reason);
}

#[tokio::test]
async fn sqlite_history_is_append_only_and_tampering_is_detected() {
    let dir = TempDir::new();
    let config = dir.config("tamper");
    let store = SqliteStore::open(&config).await.expect("opens");
    history(&store, 3).await;
    assert!(verify(&store).await.is_intact());
    store.close().await.expect("close");

    for statement in [
        "UPDATE change_log SET actor_id = 'x' WHERE seq = 2",
        "DELETE FROM change_log WHERE seq = 2",
        "UPDATE change_fields SET field = 'x' WHERE seq = 2",
        "DELETE FROM change_fields WHERE seq = 2",
        "UPDATE change_log_weeks SET week_start = 'x' WHERE seq = 1",
        "DELETE FROM change_log_weeks",
    ] {
        let refused = raw(&config, statement).await.expect_err("append-only");
        assert!(
            refused.to_string().contains("append-only"),
            "{statement}: {refused}"
        );
    }

    // An attacker who drops the triggers is still detected.
    tamper(
        &config,
        "DROP TRIGGER change_log_no_update;
         UPDATE change_log SET actor_id = 'impostor' WHERE seq = 2;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store)
        .await
        .first_broken
        .expect("column edit detected");
    assert_eq!(broken.seq, 2, "{}", broken.reason);
    store.close().await.expect("close");

    tamper(
        &config,
        "UPDATE change_log SET actor_id = (SELECT json_extract(body, '$.actor.id')) WHERE seq = 2;
         UPDATE change_log SET body = replace(body, '\"kind\":\"admin\"', '\"kind\":\"system\"'),
                               actor_kind = 'system' WHERE seq = 3;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store)
        .await
        .first_broken
        .expect("body edit detected");
    assert_eq!(broken.seq, 3, "{}", broken.reason);
    assert!(broken.reason.contains("hash"), "{}", broken.reason);
    store.close().await.expect("close");

    tamper(
        &config,
        "DROP TRIGGER change_log_weeks_no_delete;
         DROP TRIGGER change_log_no_delete;
         DROP TRIGGER change_fields_no_delete;
         DELETE FROM change_log_weeks WHERE seq >= 3;
         DELETE FROM change_fields WHERE seq >= 3;
         DELETE FROM change_log WHERE seq >= 3;
         DELETE FROM change_log_weeks WHERE seq = 1;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store)
        .await
        .first_broken
        .expect("week index edit detected");
    assert_eq!(broken.seq, 1, "{}", broken.reason);
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_removed_record_breaks_the_chain() {
    let dir = TempDir::new();
    let config = dir.config("removed");
    let store = SqliteStore::open(&config).await.expect("opens");
    history(&store, 3).await;
    store.close().await.expect("close");
    tamper(
        &config,
        "DROP TRIGGER change_log_weeks_no_delete;
         DROP TRIGGER change_log_no_delete;
         DROP TRIGGER change_fields_no_delete;
         DELETE FROM change_log_weeks WHERE seq = 2;
         DELETE FROM change_fields WHERE seq = 2;
         DELETE FROM change_log WHERE seq = 2;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store).await.first_broken.expect("gap detected");
    assert_eq!(broken.seq, 3, "{}", broken.reason);
    store.close().await.expect("close");
}

#[tokio::test]
async fn the_chain_continues_across_restarts() {
    let dir = TempDir::new();
    let config = dir.config("restarts");
    for round in 0..3 {
        let store = SqliteStore::open(&config).await.expect("opens");
        for n in 0..5 {
            commit(
                &store,
                vec![Change::PutFixedRun(fixed(&format!("r{round}-{n}")))],
            )
            .await;
        }
        let verification = verify(&store).await;
        assert!(verification.is_intact(), "round {round}");
        assert_eq!(verification.records, 1 + 5 * (round + 1));
        store.close().await.expect("close");
    }
    let store = SqliteStore::open(&config).await.expect("opens");
    let page = store
        .list_changes(&ChangeQuery::new(ChangeFilter::All))
        .await
        .expect("list");
    assert_eq!(page.records[0].id, "genesis");
    assert!(
        page.records
            .windows(2)
            .all(|pair| pair[1].prev_hash == pair[0].hash)
    );
    let last = page.records.last().expect("records");
    assert_eq!(last.rows[0].key, RowKey::FixedRun("r2-4".into()));
    assert_eq!(
        last.revision,
        store.load(&Scope::All).await.expect("load").revision
    );
    store.close().await.expect("close");
}

/// A service over `store` with a fresh id source and a fixed clock.
fn service<S: ScheduleStore>(
    store: S,
) -> kanade::domain::scheduler::SchedulerService<S, Ids, Fixed> {
    kanade::domain::scheduler::SchedulerService::new(store, Ids::default(), Fixed)
}

#[derive(Default)]
struct Ids(u64);

impl kanade::domain::ids::IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{}", self.0)
    }
}

struct Fixed;

impl kanade::domain::scheduler::Clock for Fixed {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        crate::support::at(26, 12)
    }
}

async fn revert_seq<S: ScheduleStore + ChangeHistory>(
    store: S,
    seq: u64,
) -> Result<RevertOutcome, SchedulerError> {
    let policy = ReminderPolicy {
        zone: chrono_tz::UTC,
        ping_time: chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
        countdowns: vec![60],
    };
    service(store)
        .revert_changes(
            "root",
            None,
            &[seq],
            RevertMode::Force,
            &policy,
            &BTreeSet::new(),
        )
        .await
}

#[tokio::test]
async fn a_tampered_record_is_never_reverted() {
    let memory = MemoryScheduleStore::new();
    history(&memory, 3).await;
    memory.tamper_change(2, |record| record.notices.push("forged".into()));
    assert_eq!(
        revert_seq(memory, 2).await,
        Err(SchedulerError::History(HistoryRefusal::Tampered(2)))
    );

    let dir = TempDir::new();
    let config = dir.config("tampered-revert");
    let store = SqliteStore::open(&config).await.expect("opens");
    history(&store, 3).await;
    store.close().await.expect("close");
    tamper(
        &config,
        "DROP TRIGGER change_log_no_update;
         UPDATE change_log SET body = replace(body, '\"notices\":[]', '\"notices\":[\"x\"]')
         WHERE seq = 2;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let checked = store.load_checked(2).await.expect("load");
    assert!(
        matches!(&checked, CheckedChange::Tampered(reason) if reason.contains("stored bytes")),
        "{checked:?}"
    );
    assert_eq!(
        revert_seq(store, 2).await,
        Err(SchedulerError::History(HistoryRefusal::Tampered(2)))
    );
}

#[tokio::test]
async fn a_recorded_request_replays_after_a_restart() {
    let dir = TempDir::new();
    let config = dir.config("replay");
    let request = || {
        kanade::domain::history::Origin::new(
            kanade::domain::history::Actor::member("2"),
            kanade::domain::history::Surface::PublicPortal,
        )
        .with_request_id("fixed-1")
    };
    let store = SqliteStore::open(&config).await.expect("opens");
    let mut first = service(store);
    first
        .as_origin(request())
        .add_fixed_run(fixed_run())
        .await
        .expect("applied");
    let head = first.store().history_head().await.expect("head");
    let revision = first
        .store()
        .load(&Scope::All)
        .await
        .expect("load")
        .revision;
    drop(first);

    let store = SqliteStore::open(&config).await.expect("reopens");
    let mut again = service(store);
    assert_eq!(
        again.as_origin(request()).add_fixed_run(fixed_run()).await,
        Err(SchedulerError::AlreadyApplied {
            seq: head.seq,
            revision,
        })
    );
    let mut other = fixed_run();
    other.note = Some("different".into());
    assert_eq!(
        again.as_origin(request()).add_fixed_run(other).await,
        Err(SchedulerError::IdempotencyMismatch { seq: head.seq })
    );
}

/// Boss weeks reset Wednesday 16:00 UTC.
fn schedule_policy() -> kanade::domain::schedule::SchedulePolicy {
    kanade::domain::schedule::SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::UTC,
            ping_time: chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
            countdowns: vec![60],
        },
        chrono::Weekday::Wed,
        chrono::NaiveTime::from_hms_opt(16, 0, 0).expect("valid time"),
    )
}

fn fixed_run() -> kanade::domain::schedule::NewFixedRun {
    kanade::domain::schedule::NewFixedRun {
        owner_id: "2".into(),
        channel_id: Some("900".into()),
        bosses: vec!["HFA".into()],
        weekday: chrono::Weekday::Sat,
        time: chrono::NaiveTime::from_hms_opt(20, 0, 0).expect("valid time"),
        participants: vec!["2".into()],
        note: None,
    }
}

#[tokio::test]
async fn backups_anchor_the_history_and_truncation_is_detected() {
    let dir = TempDir::new();
    let config = dir.config("anchored");
    let store = SqliteStore::open(&config).await.expect("opens");
    history(&store, 4).await;
    let head = store.history_head().await.expect("head");
    let backup = dir.path().join("anchored-backup.sqlite3");
    store.backup(&backup).await.expect("backup");
    let manifest = BackupManifest::read(&BackupManifest::path_for(&backup)).expect("manifest");
    assert_eq!(manifest.history_head, head, "the manifest anchors the head");
    assert_eq!(manifest.schema_version, 12);
    history(&store, 2).await;
    store.close().await.expect("close");

    SqliteStore::open_with_anchor(&config, &manifest.history_head)
        .await
        .expect("the anchor is still in the chain")
        .close()
        .await
        .expect("close");

    // Cut the tail back past the anchor: the chain itself still verifies.
    tamper(
        &config,
        &format!(
            "DROP TRIGGER change_log_weeks_no_delete;
             DROP TRIGGER change_log_no_delete;
             DROP TRIGGER change_fields_no_delete;
             DELETE FROM change_log_weeks WHERE seq >= {seq};
             DELETE FROM change_fields WHERE seq >= {seq};
             DELETE FROM change_log WHERE seq >= {seq};",
            seq = head.seq
        ),
    )
    .await;
    let store = SqliteStore::open(&config)
        .await
        .expect("opens without an anchor");
    assert!(
        verify(&store).await.is_intact(),
        "truncation alone is invisible"
    );
    store.close().await.expect("close");
    let refused = SqliteStore::open_with_anchor(&config, &manifest.history_head)
        .await
        .err()
        .expect("refused");
    assert!(
        matches!(&refused, SqliteStoreError::AnchorMissing { anchor } if *anchor == head),
        "{refused}"
    );
}

#[tokio::test]
async fn rows_from_before_the_history_blame_as_unknown() {
    use kanade::domain::history::{BlameTarget, blame};

    let dir = TempDir::new();
    let config = dir.config("imported");
    SqliteStore::open(&config)
        .await
        .expect("opens")
        .close()
        .await
        .expect("close");
    // Rows written outside the history, as an import would.
    tamper(
        &config,
        "INSERT INTO fixed_runs (id, owner_id, channel_id, bosses, weekday, time, participants, note)
         VALUES ('f-old', '42', '900', '[\"HFA\"]', 5, '20:00', '[\"1\"]', NULL);
         INSERT INTO runs (id, fixed_run_id, channel_id, week_start, datetime, bosses,
             participants, status, source)
         VALUES ('r-old', 'f-old', '900', '2026-08-26T16:00:00+00:00',
             '2026-08-29T12:00:00+00:00', '[\"HFA\"]', '[\"1\"]', 'planned', 'fixed');",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let run = blame(&store, &BlameTarget::Run("r-old".into()))
        .await
        .expect("blame")
        .expect("run");
    assert!(
        run.lines.iter().all(|line| line.last.is_none()),
        "unknown (before history): {run:?}"
    );
    let mut service = service(store);
    service
        .as_origin(kanade::domain::history::Origin::for_tests())
        .set_rsvp(
            "r-old",
            "1",
            kanade::domain::schedule::RsvpState::Yes,
            kanade::domain::schedule::RsvpSource::Chat,
        )
        .await
        .expect("rsvp");
    let run = blame(service.store(), &BlameTarget::Run("r-old".into()))
        .await
        .expect("blame")
        .expect("run");
    let known: Vec<&str> = run
        .lines
        .iter()
        .filter(|line| line.last.is_some())
        .map(|line| line.field.as_str())
        .collect();
    assert_eq!(known, ["rsvp:1"]);
    let timing = blame(service.store(), &BlameTarget::FixedRun("f-old".into()))
        .await
        .expect("blame")
        .expect("timing");
    assert!(timing.lines.iter().all(|line| line.last.is_none()));
}

#[tokio::test]
async fn blame_index_and_checkpoints_are_tamper_evident() {
    use kanade::domain::history::Checkpoints;

    let dir = TempDir::new();
    let config = dir.config("index");
    let store = SqliteStore::open(&config).await.expect("opens");
    history(&store, 2).await;
    let owner = service(store);
    owner
        .create_checkpoint(
            "root",
            "pinned",
            crate::support::at(26, 16),
            &schedule_policy(),
        )
        .await
        .expect("checkpoint");
    drop(owner);

    for statement in [
        "UPDATE checkpoints SET name = 'moved'",
        "DELETE FROM checkpoints",
    ] {
        let refused = raw(&config, statement).await.expect_err("immutable");
        assert!(
            refused.to_string().contains("immutable"),
            "{statement}: {refused}"
        );
    }

    tamper(
        &config,
        "DROP TRIGGER checkpoints_no_update;
         UPDATE checkpoints
            SET hash = (CASE WHEN substr(hash, 1, 1) = '0' THEN '1' ELSE '0' END) || substr(hash, 2)
          WHERE name = 'pinned';",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store)
        .await
        .first_broken
        .expect("checkpoint edit detected");
    assert!(broken.reason.contains("checkpoint"), "{}", broken.reason);
    let checkpoint = store
        .load_checkpoint("pinned")
        .await
        .expect("load")
        .expect("exists");
    let policy = ReminderPolicy {
        zone: chrono_tz::UTC,
        ping_time: chrono::NaiveTime::from_hms_opt(9, 0, 0).expect("valid time"),
        countdowns: vec![60],
    };
    let refused = service(store)
        .restore_to_checkpoint(
            "root",
            None,
            "pinned",
            RevertMode::Strict,
            &policy,
            &BTreeSet::new(),
        )
        .await;
    assert_eq!(
        refused,
        Err(SchedulerError::History(HistoryRefusal::Tampered(
            checkpoint.head.seq
        )))
    );

    tamper(
        &config,
        "DROP TRIGGER change_fields_no_delete;
         DELETE FROM change_fields WHERE seq = 1;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store)
        .await
        .first_broken
        .expect("index edit detected");
    assert_eq!(broken.seq, 1, "{}", broken.reason);
    assert!(broken.reason.contains("blame index"), "{}", broken.reason);
}

#[tokio::test]
async fn upgrading_to_the_blame_index_backfills_earlier_records() {
    use kanade::domain::history::{BlameTarget, blame};

    let dir = TempDir::new();
    let config = dir.config("backfill");
    let mut owner = service(SqliteStore::open(&config).await.expect("opens"));
    owner
        .as_origin(kanade::domain::history::Origin::for_tests())
        .add_fixed_run(fixed_run())
        .await
        .expect("fixed run");
    let fixed_id = owner
        .store()
        .load(&Scope::All)
        .await
        .expect("load")
        .fixed_runs[0]
        .id
        .clone();
    let added = owner.store().history_head().await.expect("head");
    drop(owner);
    // Roll the file back to what a version-3 build left behind.
    tamper(
        &config,
        "DROP TABLE notice_outbox;
         DROP INDEX delivery_attempts_dedupe_key;
         DROP TABLE proposal_cards;
         DROP TABLE member_aliases;
         ALTER TABLE members DROP COLUMN aliases;
         ALTER TABLE members DROP COLUMN reply_style;
         ALTER TABLE members DROP COLUMN roles;
         ALTER TABLE members DROP COLUMN is_guild_admin;
         DROP TABLE web_sessions;
         DROP TABLE draft_proposals;
         DROP TABLE self_service_tips;
         DROP TABLE chat_allowance_overrides;
         DROP TABLE rescan_jobs;
         DROP TABLE chat_tools;
         DROP TABLE chat_rounds;
         DROP TABLE chat_interactions;
         DROP TABLE extraction_members;
         DROP TABLE extractions;
         DROP TABLE messages;
         DROP TABLE run_status_pins;
         DROP TABLE run_attendance;
         DROP TABLE standing_answers;
         ALTER TABLE fixed_runs DROP COLUMN attendance_default;
         DROP TABLE checkpoints;
         DROP TABLE change_fields;
         DROP TABLE draft_requests;
         DROP TABLE draft_events;
         DROP TABLE draft_ops;
         DROP TABLE drafts;
         DELETE FROM schema_migrations WHERE version >= 4;
         UPDATE store_meta SET schema_version = 3;",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("migrates");
    assert_eq!(store.schema_version().await.expect("version"), 12);
    assert!(
        verify(&store).await.is_intact(),
        "the backfilled index verifies"
    );
    let timing = blame(&store, &BlameTarget::FixedRun(fixed_id))
        .await
        .expect("blame")
        .expect("timing");
    assert!(
        timing
            .lines
            .iter()
            .all(|line| line.last.as_ref().map(|last| last.seq) == Some(added.seq)),
        "pre-0004 changes are attributed: {timing:?}"
    );
    store.close().await.expect("close");
}

#[tokio::test]
async fn a_checkpoint_with_a_forged_revision_is_refused() {
    use kanade::domain::history::Checkpoints;

    let dir = TempDir::new();
    let config = dir.config("revision");
    let store = SqliteStore::open(&config).await.expect("opens");
    history(&store, 2).await;
    let owner = service(store);
    owner
        .create_checkpoint(
            "root",
            "pinned",
            crate::support::at(26, 16),
            &schedule_policy(),
        )
        .await
        .expect("checkpoint");
    drop(owner);
    tamper(
        &config,
        "DROP TRIGGER checkpoints_no_update;
         UPDATE checkpoints SET revision = 0 WHERE name = 'pinned';",
    )
    .await;
    let store = SqliteStore::open(&config).await.expect("reopens");
    let broken = verify(&store).await.first_broken.expect("detected");
    assert!(broken.reason.contains("checkpoint"), "{}", broken.reason);
    let checkpoint = store
        .load_checkpoint("pinned")
        .await
        .expect("load")
        .expect("exists");
    let refused = service(store)
        .restore_to_checkpoint(
            "root",
            None,
            "pinned",
            RevertMode::Strict,
            &schedule_policy().reminders,
            &BTreeSet::new(),
        )
        .await;
    assert_eq!(
        refused,
        Err(SchedulerError::History(HistoryRefusal::Tampered(
            checkpoint.head.seq
        )))
    );
}
