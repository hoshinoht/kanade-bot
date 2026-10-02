//! `kanade import v4` against a synthetic v4-shaped snapshot (v4 DDL, WAL)
//! and a temp v5 store. Never reads a real v4 database.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use kanade::domain::history::{Actor, ChangeFilter, ChangeHistory, ChangeQuery, Surface};
use kanade::domain::model_log::{
    ChatFilter, ChatOutcome, ExtractionFilter, ExtractionOutcome, ModelLogStore,
};
use kanade::domain::notify::NoticeOutbox;
use kanade::domain::scheduler::{ScheduleStore, Scope};
use kanade::import::v4::{Options, Report, run};
use kanade::infrastructure::store::{SqliteStore, SqliteStoreConfig};
use kanade::runtime::config::{FileSettings, ImportConfig, StoreSettings};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
use sqlx::{ConnectOptions, Connection};

const SECRET: &str = "SECRET-MESSAGE-TEXT";
const NAME: &str = "SecretMemberName";

const FIXED_A: &str = "0a1b2c3d-0000-4000-8000-000000000001";
const FIXED_B: &str = "0a1b2c3d-0000-4000-8000-000000000002";
const FIXED_C: &str = "0a1b2c3d-0000-4000-8000-000000000003";
const BAD_BOSS: &str = "0a1b2c3d-0000-4000-8000-00000000000a";
const BAD_PARTY: &str = "0a1b2c3d-0000-4000-8000-00000000000b";
const BAD_DAY: &str = "0a1b2c3d-0000-4000-8000-00000000000c";
const BAD_CHANNEL: &str = "0a1b2c3d-0000-4000-8000-00000000000d";

/// v4 `SCHEMA_SQL` (schema 16) for the tables the import reads, plus
/// `members` and `amendments`, which it must ignore.
const V4_DDL: &str = r#"
CREATE TABLE members (
    user_id TEXT PRIMARY KEY, display_name TEXT NOT NULL DEFAULT '', nickname TEXT,
    aliases TEXT NOT NULL DEFAULT '[]', has_role INTEGER NOT NULL DEFAULT 0,
    ping_level TEXT NOT NULL DEFAULT 'essential', reply_style TEXT, updated_at TEXT NOT NULL
);
CREATE TABLE fixed_runs (
    id TEXT PRIMARY KEY, owner_id TEXT NOT NULL, channel_id TEXT, bosses TEXT NOT NULL,
    weekday INTEGER NOT NULL, time TEXT NOT NULL, participants TEXT NOT NULL, note TEXT,
    created_at TEXT NOT NULL
);
CREATE TABLE amendments (
    id TEXT PRIMARY KEY, week_start TEXT NOT NULL, kind TEXT NOT NULL,
    bosses TEXT NOT NULL DEFAULT '[]', run_id TEXT, new_datetime TEXT,
    participants TEXT NOT NULL DEFAULT '[]', status TEXT NOT NULL DEFAULT 'proposed',
    confidence REAL, evidence_msg_ids TEXT NOT NULL DEFAULT '[]', proposal_message_id TEXT,
    created_at TEXT NOT NULL, channel_id TEXT, is_question INTEGER NOT NULL DEFAULT 0,
    rsvp TEXT, day_ref TEXT, time_ref TEXT, summary TEXT, payload TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE messages (
    id TEXT PRIMARY KEY, channel_id TEXT NOT NULL, author_id TEXT NOT NULL,
    created_at TEXT NOT NULL, content TEXT NOT NULL, processed_at TEXT
);
CREATE TABLE extractions (
    id TEXT PRIMARY KEY, at TEXT NOT NULL, model TEXT NOT NULL, prompt TEXT NOT NULL,
    raw_response TEXT NOT NULL, latency_ms INTEGER, message_ids TEXT NOT NULL DEFAULT '[]',
    amendment_ids TEXT NOT NULL DEFAULT '[]'
);
CREATE TABLE chat_interactions (
    id TEXT PRIMARY KEY, at TEXT NOT NULL, channel_id TEXT, message_id TEXT, author_id TEXT,
    model TEXT NOT NULL DEFAULT '', question TEXT NOT NULL DEFAULT '',
    reply TEXT NOT NULL DEFAULT '', outcome TEXT NOT NULL DEFAULT 'answered', error TEXT,
    rounds INTEGER NOT NULL DEFAULT 0, latency_ms INTEGER, model_ms INTEGER, tools_ms INTEGER,
    prompt_tokens INTEGER, completion_tokens INTEGER, tool_calls TEXT NOT NULL DEFAULT '[]',
    model_rounds TEXT NOT NULL DEFAULT '[]'
);
"#;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 25, 4, 0, 0).unwrap()
}

/// v4 `to_iso`: Python `isoformat()` in UTC.
fn iso(days_ago: i64) -> String {
    kanade::domain::time::to_iso(&(now() - Duration::days(days_ago))).unwrap()
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        use std::os::unix::fs::DirBuilderExt;
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let root = base.join(format!("kanade-import-{}", uuid::Uuid::new_v4()));
        for dir in [&root, &root.join("locks"), &root.join("v4")] {
            std::fs::DirBuilder::new().mode(0o700).create(dir).unwrap();
        }
        let fixture = Self { root };
        fixture.write_snapshot().await;
        fixture
    }

    fn snapshot(&self) -> PathBuf {
        self.root.join("v4").join("snapshot.sqlite3")
    }

    fn store_config(&self) -> SqliteStoreConfig {
        SqliteStoreConfig {
            db_path: self.root.join("kanade.sqlite3"),
            owner_lock_dir: self.root.join("locks"),
        }
    }

    fn config(&self) -> ImportConfig {
        let store = self.store_config();
        ImportConfig {
            timezone: chrono_tz::Asia::Kuala_Lumpur,
            store: StoreSettings {
                db_path: store.db_path,
                owner_lock_dir: store.owner_lock_dir,
            },
            files: FileSettings {
                catalog_file: Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml"),
                knowledge_dir: None,
                persona_dir: PathBuf::from("config/personas"),
            },
        }
    }

    fn options(&self, apply: bool, since: Option<NaiveDate>) -> Options {
        Options {
            from: self.snapshot(),
            since,
            apply,
            refresh_logs: false,
        }
    }

    async fn import(&self, apply: bool, since: Option<NaiveDate>) -> Report {
        run(&self.options(apply, since), &self.config(), now())
            .await
            .expect("import")
    }

    async fn write_snapshot(&self) {
        let mut conn = SqliteConnectOptions::new()
            .filename(self.snapshot())
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .connect()
            .await
            .unwrap();
        sqlx::raw_sql(V4_DDL).execute(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO members (user_id, display_name, has_role, updated_at) VALUES ('111111111111111111', ?1, 1, ?2)")
            .bind(NAME)
            .bind(iso(1))
            .execute(&mut conn)
            .await
            .unwrap();
        let fixed = [
            (
                FIXED_A,
                r#"["XLotus"]"#,
                0,
                "21:00",
                r#"["111111111111111111","222222222222222222"]"#,
                Some("900000000000000001"),
            ),
            (
                FIXED_B,
                r#"["HSeren","XLotus"]"#,
                3,
                "20:30",
                r#"["222222222222222222"]"#,
                None,
            ),
            (
                FIXED_C,
                r#"["HSeren"]"#,
                6,
                "22:00",
                r#"["111111111111111111","111111111111111111"]"#,
                Some("900000000000000002"),
            ),
            (
                BAD_BOSS,
                r#"["HNobody"]"#,
                1,
                "21:00",
                r#"["111111111111111111"]"#,
                None,
            ),
            (
                BAD_PARTY,
                r#"["XLotus"]"#,
                1,
                "21:00",
                r#"["not-a-user"]"#,
                None,
            ),
            (
                BAD_DAY,
                r#"["XLotus"]"#,
                9,
                "21:00",
                r#"["111111111111111111"]"#,
                None,
            ),
            (
                BAD_CHANNEL,
                r#"["XLotus"]"#,
                2,
                "21:00",
                r#"["111111111111111111"]"#,
                Some("general"),
            ),
        ];
        for (id, bosses, weekday, time, participants, channel) in fixed {
            sqlx::query("INSERT INTO fixed_runs VALUES (?1, '111111111111111111', ?2, ?3, ?4, ?5, ?6, ?7, ?8)")
                .bind(id)
                .bind(channel)
                .bind(bosses)
                .bind(weekday)
                .bind(time)
                .bind(participants)
                .bind(format!("{SECRET} note"))
                .bind(iso(200))
                .execute(&mut conn)
                .await
                .unwrap();
        }
        for (id, channel, days) in [
            ("m1", "900000000000000001", 1),
            ("m2", "900000000000000001", 1),
            ("m3", "900000000000000001", 1),
            ("m4", "900000000000000002", 10),
            ("m5", "900000000000000002", 2),
        ] {
            sqlx::query("INSERT INTO messages VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
                .bind(id)
                .bind(channel)
                .bind(if id == "m3" {
                    "222222222222222222"
                } else {
                    "111111111111111111"
                })
                .bind(iso(days))
                .bind(format!("{SECRET} {id}"))
                .bind(if id == "m1" { None } else { Some(iso(days)) })
                .execute(&mut conn)
                .await
                .unwrap();
        }
        let rounds = r#"[{"round":1,"content":"calling","thinking":"t","requested_tools":["get_schedule"]},{"round":2,"content":"done","thinking":null,"requested_tools":[]}]"#;
        let calls = r#"[{"name":"get_schedule","round":1,"arguments":"","output":"x","ms":3,"outcome":"ok","created":[],"posted":[]}]"#;
        for (id, at, message, outcome, error, reply) in [
            ("c1", iso(1), "m1", "answered", None, "hello"),
            ("c2", iso(2), "m9", "failed", Some("provider down"), ""),
            ("c3", iso(3), "m1", "failed", None, ""),
            ("c4", iso(120), "m1", "answered", None, "old"),
            ("c5", "yesterday".to_owned(), "m1", "answered", None, "bad"),
        ] {
            sqlx::query(
                "INSERT INTO chat_interactions (id, at, channel_id, message_id, author_id, model, question, reply, outcome, error, rounds, latency_ms, model_ms, tools_ms, prompt_tokens, completion_tokens, tool_calls, model_rounds) \
                 VALUES (?1, ?2, '900000000000000001', ?3, '111111111111111111', 'chat-model', ?4, ?5, ?6, ?7, 2, 1500, 1200, 3, 900, ?10, ?8, ?9)",
            )
            .bind(id)
            .bind(at)
            .bind(message)
            .bind(format!("{SECRET} question"))
            .bind(format!("{SECRET} {reply}"))
            .bind(outcome)
            .bind(error)
            .bind(if id == "c1" { calls } else { "[]" })
            .bind(if id == "c1" { rounds } else { "[]" })
            // v4 totals may be half a pair; c3 keeps only the prompt side.
            .bind(if id == "c3" { None } else { Some(40) })
            .execute(&mut conn)
            .await
            .unwrap();
        }
        for (id, days, messages, amendments, raw) in [
            ("e1", 1, r#"["m2","m3"]"#, r#"["a1"]"#, r#"{"changes":[1]}"#),
            ("e2", 10, r#"["m4"]"#, "[]", r#"{"changes":[]}"#),
            ("e3", 5, "[]", "[]", "timeout talking to the model"),
            ("e4", 100, r#"["m5"]"#, "[]", "{}"),
        ] {
            sqlx::query(
                "INSERT INTO extractions VALUES (?1, ?2, 'extract-model', ?3, ?4, 700, ?5, ?6)",
            )
            .bind(id)
            .bind(iso(days))
            .bind(format!("{SECRET} prompt"))
            .bind(raw)
            .bind(messages)
            .bind(amendments)
            .execute(&mut conn)
            .await
            .unwrap();
        }
        conn.close().await.unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn digest(path: &Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .to_vec()
}

async fn open(fixture: &Fixture) -> SqliteStore {
    SqliteStore::open(&fixture.store_config()).await.unwrap()
}

fn skipped(report: &Report) -> Vec<(&str, &str)> {
    report
        .fixed_skipped
        .iter()
        .map(|(id, reason)| (id.as_str(), *reason))
        .collect()
}

#[tokio::test]
async fn a_dry_run_reports_without_creating_or_writing_the_store() {
    let fixture = Fixture::new().await;
    let report = fixture.import(false, None).await;
    assert!(!fixture.store_config().db_path.exists());
    assert!(!report.applied);
    assert_eq!(report.fixed_runs.added, 3);
    assert_eq!(report.fixed_runs.skipped_total(), 4);
    assert_eq!(report.materialised, None);
    assert_eq!((report.chats.added, report.extractions.added), (3, 3));

    // Against an existing store: still nothing written.
    open(&fixture).await.close().await.unwrap();
    let again = fixture.import(false, None).await;
    assert_eq!(again.fixed_runs, report.fixed_runs);
    let store = open(&fixture).await;
    let snapshot = store.load(&Scope::All).await.unwrap();
    assert!(snapshot.fixed_runs.is_empty() && snapshot.runs.is_empty());
    assert_eq!(store.count_changes(&ChangeFilter::All).await.unwrap(), 0);
    let chats = store
        .list_chats(&ChatFilter {
            limit: 50,
            ..ChatFilter::default()
        })
        .await
        .unwrap();
    assert!(chats.items.is_empty());
    assert!(
        store
            .messages_by_ids(&["m1".into()])
            .await
            .unwrap()
            .is_empty()
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn apply_imports_fixed_runs_logs_and_messages_and_a_second_apply_adds_nothing() {
    let fixture = Fixture::new().await;
    let report = fixture.import(true, None).await;
    assert_eq!((report.fixed_runs.added, report.fixed_runs.present), (3, 0));
    assert_eq!(
        skipped(&report),
        [
            (BAD_BOSS, "unknown_boss"),
            (BAD_PARTY, "bad_participants"),
            (BAD_CHANNEL, "bad_channel"),
            (BAD_DAY, "bad_weekday"),
        ]
    );
    let created = report.materialised.expect("materialised");
    assert!(created > 0);
    assert_eq!(report.chats.added, 3);
    assert_eq!(report.chats.skipped.get("outside_window"), Some(&1));
    assert_eq!(report.chats.skipped.get("bad_timestamp"), Some(&1));
    assert_eq!(report.extractions.added, 3);
    assert_eq!(report.extractions.skipped.get("outside_window"), Some(&1));
    // m1 (chat), m2+m3 (e1), m4 (e2); m9 is not in the snapshot, m5 only
    // referenced by an out-of-window extraction.
    assert_eq!(report.messages.added, 4);
    assert_eq!(report.messages.skipped.get("not_in_snapshot"), Some(&1));

    let store = open(&fixture).await;
    let snapshot = store.load(&Scope::All).await.unwrap();
    let mut ids: Vec<&str> = snapshot.fixed_runs.iter().map(|f| f.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, [FIXED_A, FIXED_B, FIXED_C]);
    let c = snapshot
        .fixed_runs
        .iter()
        .find(|f| f.id == FIXED_C)
        .unwrap();
    assert_eq!(c.participants, ["111111111111111111"]);
    assert_eq!(snapshot.runs.len(), created);
    assert!(snapshot.runs.iter().all(|run| run.fixed_run_id.is_some()));

    let import = Actor::system("import");
    let records = store
        .list_changes(&ChangeQuery::new(ChangeFilter::All))
        .await
        .unwrap()
        .records;
    let records: Vec<_> = records.into_iter().filter(|r| r.seq > 0).collect();
    // One record per timing, then one materialisation.
    assert_eq!(records.len(), 4);
    assert!(
        records
            .iter()
            .all(|r| r.origin.actor == import && r.origin.surface == Surface::Import)
    );
    assert!(
        store.outbox_notices().await.unwrap().is_empty(),
        "imported history must not enqueue Discord notices"
    );
    assert_eq!(
        records[0].origin.request_id.as_deref(),
        Some(format!("v4-fixed:{}", ids_in_import_order()[0]).as_str())
    );

    let chat = store.load_chat("v4-c1").await.unwrap().unwrap();
    assert_eq!(chat.outcome, ChatOutcome::Answered);
    assert_eq!(chat.at, now() - Duration::days(1));
    assert_eq!(
        (chat.request_count, chat.latency_ms, chat.prompt_tokens),
        (2, Some(1500), Some(900))
    );
    assert_eq!(chat.rounds.len(), 2);
    assert_eq!(chat.rounds[0].tools, ["get_schedule"]);
    assert_eq!(chat.rounds[0].tool_calls.as_array().unwrap().len(), 1);
    assert!(chat.rounds[1].tool_calls.as_array().unwrap().is_empty());
    assert_eq!(chat.rounds[0].model, "chat-model");
    assert_eq!(chat.completion_tokens, Some(40));
    assert!(
        chat.rounds.iter().all(|round| (
            round.prompt_tokens,
            round.completion_tokens,
            round.prompt_estimate
        ) == (None, None, None)),
        "v4 rounds carry no usage"
    );
    let half = store.load_chat("v4-c3").await.unwrap().unwrap();
    assert_eq!(
        (half.prompt_tokens, half.completion_tokens),
        (Some(900), None),
        "v4 interaction totals are kept as recorded, half pairs included"
    );
    let failed = store.load_chat("v4-c2").await.unwrap().unwrap();
    assert_eq!(failed.outcome, ChatOutcome::Error);
    assert_eq!(failed.error.as_deref(), Some("provider down"));
    assert_eq!(
        store.load_chat("v4-c3").await.unwrap().unwrap().outcome,
        ChatOutcome::Unknown
    );
    assert!(store.load_chat("v4-c4").await.unwrap().is_none());

    let proposed = store.load_extraction("v4-e1").await.unwrap().unwrap();
    assert_eq!(proposed.outcome, ExtractionOutcome::Proposed);
    assert_eq!(proposed.channel_id.as_deref(), Some("900000000000000001"));
    assert_eq!(
        proposed.member_ids,
        ["111111111111111111", "222222222222222222"]
    );
    assert_eq!(
        (proposed.latency_ms, proposed.proposal_ids.len()),
        (Some(700), 1)
    );
    assert_eq!(
        (
            proposed.prompt_tokens,
            proposed.completion_tokens,
            proposed.prompt_estimate
        ),
        (None, None, None),
        "v4 extractions carry no usage"
    );
    assert_eq!(
        store
            .load_extraction("v4-e2")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        ExtractionOutcome::NoChange
    );
    assert_eq!(
        store
            .load_extraction("v4-e3")
            .await
            .unwrap()
            .unwrap()
            .outcome,
        ExtractionOutcome::Unknown
    );
    let messages = store
        .messages_by_ids(&[
            "m1".into(),
            "m2".into(),
            "m3".into(),
            "m4".into(),
            "m5".into(),
        ])
        .await
        .unwrap();
    assert_eq!(messages.len(), 4);
    // Never backlog for the v5 extractor, even m1 which v4 left unprocessed.
    assert!(messages.iter().all(|m| m.processed_at.is_some()));
    let pending = store
        .channel_messages("900000000000000001", now() - Duration::days(30), true)
        .await
        .unwrap();
    assert!(pending.is_empty());
    store.close().await.unwrap();

    let again = fixture.import(true, None).await;
    assert_eq!((again.fixed_runs.added, again.fixed_runs.present), (0, 3));
    assert_eq!(again.materialised, Some(0));
    assert_eq!((again.chats.added, again.chats.present), (0, 3));
    assert_eq!((again.extractions.added, again.extractions.present), (0, 3));
    assert_eq!((again.messages.added, again.messages.present), (0, 4));
    let store = open(&fixture).await;
    assert_eq!(store.count_changes(&ChangeFilter::All).await.unwrap(), 4);
    let listed = store
        .list_extractions(&ExtractionFilter {
            limit: 50,
            ..ExtractionFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.items.len(), 3);
    store.close().await.unwrap();
}

/// Valid timings in snapshot order (`weekday, time, id`).
fn ids_in_import_order() -> [&'static str; 3] {
    [FIXED_A, FIXED_B, FIXED_C]
}

#[tokio::test]
async fn since_narrows_the_log_window() {
    let fixture = Fixture::new().await;
    let since = (now() - Duration::days(4)).date_naive();
    let report = fixture.import(true, Some(since)).await;
    assert_eq!(report.chats.added, 3);
    assert_eq!(report.extractions.added, 1);
    assert_eq!(report.extractions.skipped.get("outside_window"), Some(&3));
    assert_eq!(report.fixed_runs.added, 3, "fixed runs ignore the window");
    let store = open(&fixture).await;
    assert!(store.load_extraction("v4-e1").await.unwrap().is_some());
    assert!(store.load_extraction("v4-e3").await.unwrap().is_none());
    store.close().await.unwrap();
}

#[tokio::test]
async fn the_snapshot_is_never_modified_and_output_has_no_text_or_names() {
    let fixture = Fixture::new().await;
    let before = digest(&fixture.snapshot());
    let dry = fixture.import(false, None).await.to_string();
    let applied = fixture.import(true, None).await.to_string();
    assert_eq!(digest(&fixture.snapshot()), before);
    let side_files: Vec<_> = std::fs::read_dir(fixture.root.join("v4"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(side_files, ["snapshot.sqlite3"]);
    for output in [&dry, &applied] {
        for secret in [
            SECRET,
            NAME,
            "question",
            "hello",
            "provider down",
            "HNobody",
        ] {
            assert!(!output.contains(secret), "{secret} in {output}");
        }
    }
    assert!(dry.contains("dry run") && dry.contains("fixed runs: would add 3"));
    assert!(applied.contains("fixed runs: added 3"));
    assert!(applied.contains(&format!("skipped fixed run {BAD_BOSS}: unknown_boss")));
}

#[tokio::test]
async fn refuses_a_non_v4_source_and_the_store_itself() {
    let fixture = Fixture::new().await;
    open(&fixture).await.close().await.unwrap();
    let mut options = fixture.options(true, None);
    options.from = fixture.store_config().db_path;
    let error = run(&options, &fixture.config(), now()).await.unwrap_err();
    assert_eq!(error.to_string(), "--from names the v5 store itself");

    let other = fixture.root.join("v4").join("other.sqlite3");
    let mut conn = SqliteConnectOptions::new()
        .filename(&other)
        .create_if_missing(true)
        .connect()
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TABLE fixed_runs (id TEXT)")
        .execute(&mut conn)
        .await
        .unwrap();
    conn.close().await.unwrap();
    options.from = other;
    let error = run(&options, &fixture.config(), now()).await.unwrap_err();
    assert!(
        error.to_string().starts_with("not a v4 database"),
        "{error}"
    );
    options.from = fixture.root.join("v4").join("missing.sqlite3");
    assert!(run(&options, &fixture.config(), now()).await.is_err());
    assert!(!options.from.exists());
}

#[tokio::test]
async fn cannot_run_while_another_owner_holds_the_store() {
    let fixture = Fixture::new().await;
    let serving = open(&fixture).await;
    for apply in [false, true] {
        let error = run(&fixture.options(apply, None), &fixture.config(), now())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("already owned"), "{error}");
    }
    let snapshot = serving.load(&Scope::All).await.unwrap();
    assert!(snapshot.fixed_runs.is_empty());
    serving.close().await.unwrap();
}

/// A v4 chat whose rounds left `requested_tools` empty (the live shape).
async fn add_unlisted_tool_chat(fixture: &Fixture) {
    let mut conn = SqliteConnectOptions::new()
        .filename(fixture.snapshot())
        .journal_mode(SqliteJournalMode::Wal)
        .connect()
        .await
        .unwrap();
    let rounds = r#"[{"round":1,"content":"let me check","thinking":"","requested_tools":[]},{"round":2,"content":"Use the bind.","thinking":"","requested_tools":[]}]"#;
    let calls = r#"[{"name":"get_boss_strategy","round":1,"arguments":"{\"boss\":\"Kalos\"}","output":"Kalos notes","ms":12,"outcome":"ok"},{"name":"get_boss_strategy","round":1,"arguments":"{}","output":"again","ms":4,"outcome":"ok"},{"name":"get_schedule","round":1,"arguments":"","output":"none","ms":1,"outcome":"ok"}]"#;
    sqlx::query(
        "INSERT INTO chat_interactions (id, at, channel_id, message_id, author_id, model, question, reply, outcome, rounds, tool_calls, model_rounds) \
         VALUES ('c6', ?1, '900000000000000001', 'm1', '111111111111111111', 'chat-model', 'kalos?', 'Use the bind.', 'answered', 2, ?2, ?3)",
    )
    .bind(iso(1))
    .bind(calls)
    .bind(rounds)
    .execute(&mut conn)
    .await
    .unwrap();
    conn.close().await.unwrap();
}

fn by_tool(tool: &str) -> ChatFilter {
    ChatFilter {
        tool: Some(tool.into()),
        limit: 50,
        ..ChatFilter::default()
    }
}

#[tokio::test]
async fn call_names_stand_in_for_empty_requested_tools() {
    let fixture = Fixture::new().await;
    add_unlisted_tool_chat(&fixture).await;
    fixture.import(true, None).await;
    let store = open(&fixture).await;
    let chat = store.load_chat("v4-c6").await.unwrap().unwrap();
    assert_eq!(chat.rounds[0].tools, ["get_boss_strategy", "get_schedule"]);
    assert!(chat.rounds[1].tools.is_empty());
    assert_eq!(chat.rounds[0].tool_calls[0]["output"], "Kalos notes");
    assert_eq!(chat.rounds[0].tool_calls[0]["ms"], 12);
    let found = store
        .list_chats(&by_tool("get_boss_strategy"))
        .await
        .unwrap();
    assert_eq!(found.items.len(), 1);
    assert!(
        store
            .chat_facets()
            .await
            .unwrap()
            .tools
            .contains(&"get_boss_strategy".to_owned())
    );
    store.close().await.unwrap();
}

#[tokio::test]
async fn refresh_logs_replaces_only_imported_logs_and_is_idempotent() {
    let fixture = Fixture::new().await;
    add_unlisted_tool_chat(&fixture).await;
    // The older mapping's row for c6 (no tool names) and a native v5 row.
    let store = open(&fixture).await;
    let mut stale = kanade::domain::model_log::ChatInteraction {
        id: "v4-c6".into(),
        at: now() - Duration::days(1),
        channel_id: Some("900000000000000001".into()),
        message_id: Some("m1".into()),
        member_id: Some("111111111111111111".into()),
        question: "kalos?".into(),
        reply: "stale".into(),
        outcome: ChatOutcome::Answered,
        error: None,
        clean_retry: false,
        withheld: false,
        guardrail: serde_json::json!({}),
        request_count: 2,
        latency_ms: None,
        model_ms: None,
        tools_ms: None,
        prompt_tokens: None,
        completion_tokens: None,
        rounds: vec![kanade::domain::model_log::ChatRound {
            reasoning_content: None,
            reasoning_tokens: None,
            model: "chat-model".into(),
            reasoning: None,
            finish_reason: None,
            latency_ms: None,
            tool_bundles: Vec::new(),
            tools: Vec::new(),
            tool_calls: serde_json::json!([{"name": "get_boss_strategy"}]),
            response: None,
            route: None,
            clean: false,
            prompt_tokens: None,
            completion_tokens: None,
            prompt_estimate: None,
        }],
        persona: None,
        profile: None,
        profile_source: None,
        error_code: None,
    };
    store.record_chat(stale.clone()).await.unwrap();
    stale.id = "native-1".into();
    stale.rounds[0].tools = vec!["get_schedule".into()];
    store.record_chat(stale.clone()).await.unwrap();
    assert!(
        store
            .refresh_imported_logs(std::slice::from_ref(&stale), &[])
            .await
            .is_err(),
        "a native id is refused"
    );
    store.close().await.unwrap();

    let first = fixture.import(true, None).await;
    assert_eq!((first.chats.added, first.chats.present), (3, 1));
    let store = open(&fixture).await;
    assert!(
        store
            .list_chats(&by_tool("get_boss_strategy"))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let changes = store.count_changes(&ChangeFilter::All).await.unwrap();
    let fixed = store.load(&Scope::All).await.unwrap();
    store.close().await.unwrap();

    let mut refresh = fixture.options(false, None);
    refresh.refresh_logs = true;
    let dry = run(&refresh, &fixture.config(), now()).await.unwrap();
    assert_eq!((dry.chats.replaced, dry.chats.added), (4, 0));
    assert_eq!((dry.extractions.replaced, dry.extractions.added), (3, 0));
    let text = dry.to_string();
    assert!(
        text.contains("chat logs: would add 0, would replace 4"),
        "{text}"
    );
    assert!(text.contains("not touched"), "{text}");
    let store = open(&fixture).await;
    assert_eq!(
        store.load_chat("v4-c6").await.unwrap().unwrap().reply,
        "stale",
        "a dry run writes nothing"
    );
    store.close().await.unwrap();

    refresh.apply = true;
    let applied = run(&refresh, &fixture.config(), now()).await.unwrap();
    assert_eq!((applied.chats.replaced, applied.chats.added), (4, 0));
    assert_eq!(applied.extractions.replaced, 3);
    assert!(applied.materialised.is_none());
    let store = open(&fixture).await;
    let repaired = store.load_chat("v4-c6").await.unwrap().unwrap();
    assert_eq!(repaired.reply, "Use the bind.");
    assert_eq!(
        repaired.rounds[0].tools,
        ["get_boss_strategy", "get_schedule"]
    );
    let found = store
        .list_chats(&by_tool("get_boss_strategy"))
        .await
        .unwrap();
    assert_eq!(
        found
            .items
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        ["v4-c6"]
    );
    let native = store.load_chat("native-1").await.unwrap().unwrap();
    assert_eq!(native, stale);
    assert_eq!(
        store
            .list_chats(&by_tool("get_schedule"))
            .await
            .unwrap()
            .items
            .len(),
        3,
        "native-1, v4-c1 and v4-c6"
    );
    assert_eq!(
        store.count_changes(&ChangeFilter::All).await.unwrap(),
        changes
    );
    assert_eq!(store.load(&Scope::All).await.unwrap(), fixed);
    let extraction = store.load_extraction("v4-e1").await.unwrap().unwrap();
    assert_eq!(
        (
            extraction.prompt_tokens,
            extraction.completion_tokens,
            extraction.prompt_estimate
        ),
        (None, None, None)
    );
    let half = store.load_chat("v4-c3").await.unwrap().unwrap();
    assert_eq!(
        (half.prompt_tokens, half.completion_tokens),
        (Some(900), None),
        "a refresh keeps half-pair interaction totals"
    );
    assert!(
        half.rounds
            .iter()
            .chain(&repaired.rounds)
            .all(|round| round.prompt_tokens.is_none() && round.prompt_estimate.is_none())
    );
    store.close().await.unwrap();

    let again = run(&refresh, &fixture.config(), now()).await.unwrap();
    assert_eq!((again.chats.replaced, again.extractions.replaced), (4, 3));
    let store = open(&fixture).await;
    assert_eq!(store.load_chat("v4-c6").await.unwrap().unwrap(), repaired);
    assert_eq!(
        store.load_extraction("v4-e1").await.unwrap().unwrap(),
        extraction
    );
    assert_eq!(store.chat_facets().await.unwrap().total, 5);
    store.close().await.unwrap();
}
