//! Fresh, self-removing temp directories and store fixtures.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::domain::schedule::{
    Change, ChangeSet, FixedRun, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
};
use kanade::domain::scheduler::{ScheduleStore, Scope};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};

/// A plain connection to the database at `config`, bypassing the store.
pub async fn connect(config: &SqliteStoreConfig, read_only: bool) -> sqlx::SqliteConnection {
    let options = SqliteConnectOptions::new().filename(&config.db_path);
    options
        .read_only(read_only)
        .connect()
        .await
        .expect("raw connection")
}

/// Run `sql` on the database with a plain connection.
pub async fn raw(config: &SqliteStoreConfig, sql: &str) -> Result<(), sqlx::Error> {
    let mut conn = connect(config, false).await;
    let result = sqlx::raw_sql(sql).execute(&mut conn).await.map(|_| ());
    conn.close().await?;
    result
}

/// Run `sql` on a closed database with a plain connection.
pub async fn tamper(config: &SqliteStoreConfig, sql: &str) {
    raw(config, sql).await.expect("tamper");
}

/// Create the database at `config` on the newest schema, then close it.
pub async fn create(config: &SqliteStoreConfig) {
    let store = SqliteStore::open(config).await.expect("opens");
    store.close().await.expect("close");
}

/// What the downgrade scripts below v22 all remove first: the session and
/// request ids, the later web-session columns, and the tables and columns
/// from settings changes to owner pins.
pub const LATE_SCHEMA_DROPS: &str = "
    ALTER TABLE extractions DROP COLUMN session_id;
    ALTER TABLE extractions DROP COLUMN request_ids;
    ALTER TABLE chat_interactions DROP COLUMN session_id;
    ALTER TABLE chat_rounds DROP COLUMN request_ids;
    ALTER TABLE web_sessions DROP COLUMN superseded_until;
    ALTER TABLE web_sessions DROP COLUMN client_tag;
    ALTER TABLE web_sessions DROP COLUMN device;
    ALTER TABLE web_sessions DROP COLUMN avatar_hash;
    DROP TABLE settings_changes;
    DROP TABLE rewrites;
    DROP TABLE header_overrides;
    DROP TABLE idempotency_replays;
    DROP TABLE auth_audit;
    DROP TABLE owner_requests;
    DROP TABLE run_prompts;
    DROP TABLE roster_fixed_picks;
    DROP TABLE roster_run_picks;
    DROP TABLE roster_characters;
    ALTER TABLE fixed_runs DROP COLUMN owner_pinned;
";

/// Entries of `dir` whose name marks a staging leftover.
pub fn partials(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("list")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".partial-"))
        .collect()
}

/// Create a directory with mode 0700.
pub fn private_dir(path: &Path) {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("private dir is created");
}

pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        // Canonical (macOS `/var` is a symlink) and private, as the store requires.
        let root = std::fs::canonicalize(std::env::temp_dir()).expect("temp dir");
        let path = root.join(format!("kanade-store-{}", uuid::Uuid::new_v4()));
        private_dir(&path);
        private_dir(&path.join("locks"));
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn config(&self, name: &str) -> SqliteStoreConfig {
        SqliteStoreConfig {
            db_path: self.0.join(format!("{name}.sqlite3")),
            owner_lock_dir: self.0.join("locks"),
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn at(day: u32, hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, day, hour, 0, 0)
        .single()
        .expect("valid instant")
}

pub fn fixed(id: &str) -> FixedRun {
    FixedRun {
        owner_pinned: false,
        id: id.into(),
        owner_id: "42".into(),
        channel_id: None,
        bosses: vec!["HFA".into(), "Lucid".into()],
        weekday: Weekday::Sun,
        time: NaiveTime::from_hms_opt(21, 30, 0).expect("valid time"),
        participants: vec!["1".into()],
        note: Some("note".into()),
        attendance_default: Default::default(),
        standing: Vec::new(),
    }
}

pub fn run(id: &str) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: Some("f-1".into()),
        channel_id: Some("900".into()),
        week_start: at(26, 16),
        datetime: at(30, 12),
        bosses: vec!["HFA".into()],
        participants: vec!["1".into(), "2".into()],
        status: RunStatus::AtRisk,
        source: RunSource::Fixed,
        attendance: Vec::new(),
        status_pin: None,
    }
}

pub fn reminder(id: &str, run_id: &str) -> Reminder {
    Reminder {
        id: id.into(),
        run_id: run_id.into(),
        kind: "day_of".into(),
        fire_at: at(30, 8),
        sent_at: Some(at(30, 8) + chrono::TimeDelta::microseconds(250)),
        message_id: Some("4242".into()),
    }
}

pub fn rsvp(run_id: &str) -> Rsvp {
    Rsvp {
        run_id: run_id.into(),
        user_id: "2".into(),
        state: RsvpState::Maybe,
        source: RsvpSource::Slash,
        at: at(29, 3),
    }
}

/// Commit `changes` at the store's current revision.
pub async fn commit<S: ScheduleStore>(store: &S, changes: Vec<Change>) {
    let revision = store.load(&Scope::All).await.expect("load").revision;
    store
        .commit(
            revision,
            ChangeSet { changes },
            kanade::infrastructure::store::conformance::meta(),
        )
        .await
        .expect("commit succeeds");
}

/// A timing, a run and its rows.
pub async fn seed<S: ScheduleStore>(store: &S) {
    commit(
        store,
        vec![
            Change::PutFixedRun(fixed("f-1")),
            Change::PutRun(run("r-1")),
            Change::PutReminder(reminder("m-1", "r-1")),
            Change::PutRsvp(rsvp("r-1")),
        ],
    )
    .await;
}
