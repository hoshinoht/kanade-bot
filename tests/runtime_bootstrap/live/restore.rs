//! Offline recovery proof using only invented state and two private temp roots.

use std::{os::unix::fs::PermissionsExt, time::SystemTime};

use chrono::{DateTime, Duration as TimeDelta, NaiveTime, Utc, Weekday};
use kanade::{
    domain::{
        history::{Actor, ChangeHistory, ChangeRef, Origin, Surface},
        ids::RandomIds,
        schedule::{NewRun, RunSource, RunStatus, utc_instant},
        scheduler::{Clock, ScheduleStore, SchedulerService, Scope},
        settings::RuntimeSettings,
    },
    infrastructure::store::{BackupManifest, SqliteStore, SqliteStoreConfig},
};

use super::*;

struct Running(Option<Child>);

impl Running {
    fn stop(mut self) {
        let child = self.0.as_mut().unwrap();
        assert!(
            Command::new("/bin/kill")
                .args(["-TERM", &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        assert!(child.wait_timeout(Duration::from_secs(10)).is_some());
        let output = self.0.take().unwrap().wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(r#""event":"store_closed""#), "{stderr}");
        assert!(!stderr.contains("store_dropped_unclosed"), "{stderr}");
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // An assertion failure must not leave an owner running as Live removes its root.
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Clone, Copy)]
struct FixtureClock(DateTime<Utc>);

impl Clock for FixtureClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn config(live: &Live) -> SqliteStoreConfig {
    for dir in ["db", "locks"] {
        fs::DirBuilder::new()
            .mode(0o700)
            .create(live.root.join(dir))
            .unwrap();
    }
    SqliteStoreConfig {
        db_path: live.root.join("db/kanade.sqlite3"),
        owner_lock_dir: live.root.join("locks"),
    }
}

async fn seed(config: &SqliteStoreConfig) -> ChangeRef {
    let at = DateTime::<Utc>::from_timestamp(unix_seconds(), 0).unwrap();
    let mut settings = RuntimeSettings::default();
    settings.schedule.reset_weekday = Weekday::Wed;
    settings.schedule.reset_time = NaiveTime::from_hms_opt(8, 0, 0).unwrap();
    let policy = settings.schedule_policy(chrono_tz::Asia::Kuala_Lumpur);
    let datetime = at + TimeDelta::days(8);
    let store = SqliteStore::open(config).await.unwrap();
    let mut service = SchedulerService::new(store, RandomIds, FixtureClock(at))
        .with_attendance(policy.attendance);
    let result = service
        .as_origin(Origin::new(Actor::admin("restore-drill"), Surface::Cli))
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: None,
            week_start: utc_instant(&policy.week_of(&datetime).unwrap()).unwrap(),
            datetime,
            bosses: vec!["HSeren".into()],
            participants: Vec::new(),
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await;
    let store = service.into_store();
    let head = store.history_head().await;
    store.close().await.unwrap();
    result.unwrap();
    let head = head.unwrap();
    assert!(head.seq > 0);
    head
}

fn checkpoints(live: &Live, backups: &Path, head: &ChangeRef) -> serde_json::Value {
    let port = unused_loopback_port();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let server = Running(Some(
        live.command(port, None)
            .env("KANADE_BACKUP_DIR", backups)
            .spawn()
            .unwrap(),
    ));
    assert!(ready(address).starts_with("HTTP/1.1 200"));
    let login = call(
        address,
        "POST",
        "/api/admin/auth/token",
        &[("Sec-Fetch-Site", "same-origin")],
        Some(&format!(r#"{{"token":"{ADMIN_TOKEN}"}}"#)),
    );
    assert_eq!(login.status, 200, "{}", login.body);
    let cookie = login
        .header("set-cookie")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let reply = call(
        address,
        "GET",
        "/api/admin/history/checkpoints",
        &[("Cookie", cookie)],
        None,
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["verified"]["ok"], true);
    assert_eq!(body["verified"]["head"]["seq"], head.seq);
    assert_eq!(body["verified"]["head"]["hash"], head.hash);
    assert!(body["verified"]["checked"].as_u64().unwrap() > 0);
    assert_eq!(body["verified"]["first_broken"], serde_json::Value::Null);
    assert_eq!(body["backup_dir_configured"], true);
    server.stop();
    body
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .try_into()
        .unwrap()
}

#[test]
fn binary_backup_validated_restore_and_live_restart_preserve_state_and_anchor() {
    let _serial = serial();
    let source = Live::new();
    let source_config = config(&source);
    let backups = source.root.join("backups");
    fs::DirBuilder::new().mode(0o700).create(&backups).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let head = runtime.block_on(seed(&source_config));
    assert!(
        checkpoints(&source, &backups, &head)["backups"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let before = unix_seconds();
    let output = Command::new(binary())
        .args(["backup", "--name", "drill.sqlite"])
        .env_clear()
        .env("KANADE_DB_PATH", &source_config.db_path)
        .env("KANADE_OWNER_LOCK_DIR", &source_config.owner_lock_dir)
        .env("KANADE_BACKUP_DIR", &backups)
        .output()
        .unwrap();
    let after = unix_seconds();
    assert!(output.status.success(), "{output:?}");
    let snapshot = backups.join("drill.sqlite");
    let manifest_path = BackupManifest::path_for(&snapshot);
    let manifest = BackupManifest::read(&manifest_path).unwrap();
    assert_eq!(manifest.history_head, head);
    assert!((before..=after).contains(&manifest.created_at.unwrap().timestamp()));
    for file in [&snapshot, &manifest_path] {
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let restored = Live::new();
    let restored_config = config(&restored);
    runtime.block_on(async {
        let original = SqliteStore::open(&source_config).await.unwrap();
        let original_state = original.load(&Scope::All).await;
        let original_schema = original.schema_version().await;
        original.close().await.unwrap();
        let store = SqliteStore::restore(&snapshot, &restored_config)
            .await
            .unwrap();
        let restored_state = store.load(&Scope::All).await;
        let restored_schema = store.schema_version().await;
        let verified = store.verify_history().await;
        store.close().await.unwrap();
        let original_state = original_state.unwrap();
        assert!(!original_state.runs.is_empty());
        assert_eq!(manifest.revision, original_state.revision);
        assert_eq!(manifest.schema_version, original_schema.unwrap());
        assert_eq!(restored_state.unwrap(), original_state);
        assert_eq!(restored_schema.unwrap(), manifest.schema_version);
        assert_eq!(verified.unwrap().head, Some(head.clone()));
    });
    assert_eq!(
        fs::metadata(&restored_config.db_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let checkpoint = checkpoints(&restored, &backups, &head);
    let rows = checkpoint["backups"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["file"], "drill.sqlite");
    assert_eq!(rows[0]["history_head"]["seq"], head.seq);
    assert_eq!(rows[0]["history_head"]["hash"], head.hash);
    assert_eq!(rows[0]["schema_version"], manifest.schema_version);
    assert_eq!(rows[0]["anchored"], true);
    assert_eq!(rows[0]["anchor"], "matches");
}
