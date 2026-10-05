//! A5 history against the seeded store of `reads.rs` (one seed record by
//! `admin:seed`; the pinned clock is Tue 29 Sep 12:00 KL; this boss week
//! starts Thu 24 Sep 00:00 KL = 2026-09-23T16:00:00Z). Every response is
//! validated against the (A5-extended) schemas.

use kanade::domain::history::{ChangeHistory, Surface};
use kanade::domain::scheduler::ScheduleStore;
use serde_json::{Value, json};
use sqlx::ConnectOptions;

use crate::{reads::Reads, schemas::assert_valid, support::ADMIN_HOST, support::request};

const PAGE: &str = "history.json#/$defs/HistoryPage";
const RECORD: &str = "history.json#/$defs/ChangeRecord";
const PLAN: &str = "history.json#/$defs/RevertPlan";
const THIS_WEEK: &str = "2026-09-23T16:00:00+00:00";

impl Reads {
    async fn move_kalos(&self, day: u8, time: &str) -> u64 {
        let v = self.version().await;
        self.ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": day, "time": time, "version": v}),
            "week.json#/$defs/MoveResult",
        )
        .await;
        v + 1
    }

    async fn kalos(&self) -> Value {
        let week = self.read("/api/admin/week", "week.json#/$defs/Week").await;
        week["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["id"] == "r-kalos")
            .unwrap()
            .clone()
    }

    async fn plan(&self, path: &str, body: Value, extra: &[(&str, &str)]) -> Value {
        let reply = self.call("POST", path, body, extra).await;
        assert_eq!(reply.status, 200, "{path}: {}", reply.text());
        let plan = reply.json();
        assert_valid(PLAN, path, &plan);
        plan
    }

    async fn status_of(&self, path: &str) -> (u16, String) {
        let reply = request(
            self.admin,
            "GET",
            ADMIN_HOST,
            path,
            &[("Cookie", &self.cookie)],
        )
        .await;
        assert_valid("error.json#/$defs/ApiError", path, &reply.json());
        (reply.status, reply.api_error())
    }
}

fn seqs(page: &Value) -> Vec<u64> {
    page["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["seq"].as_u64().unwrap())
        .collect()
}

/// Reminder ids are planned afresh by each rollback, so previews are
/// compared on runs, RSVPs and timings.
fn schedule_rows(plan: &Value) -> Vec<Value> {
    plan["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["key"]["table"] != "reminders")
        .cloned()
        .collect()
}

#[tokio::test]
async fn pages_newest_first_with_filters_totals_and_records() {
    let reads = Reads::new().await;
    reads.move_kalos(6, "21:00").await;
    reads.move_kalos(4, "20:00").await;
    reads.move_kalos(3, "19:00").await;
    let head = reads.version().await;
    assert_eq!(head, 4, "seed plus three moves");

    // Walk every page: newest first, genesis never listed, total constant.
    let mut listed = Vec::new();
    let mut path = "/api/admin/history?limit=2".to_owned();
    loop {
        let page = reads.read(&path, PAGE).await;
        assert_eq!(page["total"], 4);
        assert_eq!(page["head"]["seq"], head);
        listed.extend(seqs(&page));
        match page["next_before"].as_u64() {
            Some(before) => path = format!("/api/admin/history?limit=2&before={before}"),
            None => break,
        }
    }
    assert_eq!(listed, [4, 3, 2, 1]);

    let page = reads.read("/api/admin/history", PAGE).await;
    let newest = &page["records"][0];
    assert_eq!(newest["actor"], json!({"kind": "admin", "id": "token"}));
    assert_eq!(newest["surface"], "admin_portal");
    assert!(
        newest["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["key"]["table"] == "reminders"),
        "moves record their reminder rows"
    );
    let stored = reads.store.load_change(4).await.unwrap().unwrap();
    assert_eq!(newest["hash"], stored.hash.as_str());
    let one = reads.read("/api/admin/history/4", RECORD).await;
    assert_eq!(&one, newest);

    // Filters: actor, week (either form), both.
    let by_admin = reads
        .read("/api/admin/history?actor=admin:token", PAGE)
        .await;
    assert_eq!(
        (seqs(&by_admin), by_admin["total"].clone()),
        (vec![4, 3, 2], json!(3))
    );
    let seed = reads
        .read("/api/admin/history?actor=admin:seed", PAGE)
        .await;
    assert_eq!(seqs(&seed), [1]);
    let week = format!("/api/admin/history?week={}", THIS_WEEK.replace('+', "%2B"));
    let by_week = reads.read(&week, PAGE).await;
    assert_eq!(seqs(&by_week), [4, 3, 2, 1]);
    let by_date = reads.read("/api/admin/history?week=2026-09-24", PAGE).await;
    assert_eq!(seqs(&by_date), seqs(&by_week));
    let next = reads.read("/api/admin/history?week=2026-10-01", PAGE).await;
    assert_eq!((seqs(&next), next["total"].clone()), (vec![1], json!(1)));
    let both = reads
        .read("/api/admin/history?week=2026-10-01&actor=admin:token", PAGE)
        .await;
    assert_eq!((seqs(&both), both["total"].clone()), (vec![], json!(0)));
    assert_eq!(both["next_before"], Value::Null);

    for bad in [
        "week=2026-09-25",
        "week=this",
        "actor=robot:1",
        "actor=member",
        "limit=0",
        "limit=101",
        "limit=-1",
        "before=x",
        "limit=1&limit=2",
        "colour=red",
        "week=%ZZ",
    ] {
        assert_eq!(
            reads.status_of(&format!("/api/admin/history?{bad}")).await,
            (422, "invalid_query".into()),
            "{bad}"
        );
    }
    for missing in ["0", "99", "abc"] {
        assert_eq!(
            reads
                .status_of(&format!("/api/admin/history/{missing}"))
                .await,
            (404, "not_found".into()),
            "{missing}"
        );
    }
}

/// Config saves are not in the chain, so they page by time: each sits on the
/// page whose oldest record is the newest one at or before it, exactly once.
#[tokio::test]
async fn config_saves_interleave_by_time_and_appear_on_exactly_one_page() {
    use chrono::{DateTime, Duration, Utc};
    use kanade::domain::history::Actor;
    use kanade::domain::settings::{RowDiff, SettingsChange, SettingsStore};

    let reads = Reads::new().await;
    reads.move_kalos(6, "21:00").await;
    reads.move_kalos(4, "20:00").await;
    reads.move_kalos(3, "19:00").await;
    let at = |seq: u64| {
        let reads = &reads;
        async move {
            let record = reads
                .read(&format!("/api/admin/history/{seq}"), RECORD)
                .await;
            DateTime::parse_from_rfc3339(record["at"].as_str().unwrap())
                .unwrap()
                .with_timezone(&Utc)
        }
    };
    let (seed_at, moved_at) = (at(1).await, at(4).await);
    let save = |at: DateTime<Utc>, section: &str| SettingsChange {
        id: 0,
        at,
        actor: Actor::admin("token"),
        surface: Surface::AdminPortal,
        section: section.into(),
        revision: 1,
        values: [(
            "quiet_mode".to_owned(),
            RowDiff {
                from: "0".into(),
                to: "1".into(),
            },
        )]
        .into(),
    };
    for (when, section) in [
        (moved_at + Duration::hours(1), "newest"),
        (moved_at, "tied"),
        (seed_at - Duration::hours(1), "oldest"),
    ] {
        reads
            .store
            .put_settings_rows_recorded(
                vec![("quiet_mode".into(), "1".into())],
                save(when, section),
            )
            .await
            .unwrap();
    }

    let mut pages = Vec::new();
    let mut path = "/api/admin/history?limit=2".to_owned();
    loop {
        let page = reads.read(&path, PAGE).await;
        assert_eq!(page["settings_total"], 3);
        let sections: Vec<String> = page["settings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["section"].as_str().unwrap().to_owned())
            .collect();
        pages.push((seqs(&page), sections));
        match page["next_before"].as_u64() {
            Some(before) => path = format!("/api/admin/history?limit=2&before={before}"),
            None => break,
        }
    }
    assert_eq!(
        pages,
        [
            (vec![4, 3], vec!["newest".to_owned(), "tied".to_owned()]),
            (vec![2, 1], vec!["oldest".to_owned()]),
        ]
    );
    // One page holding everything lists every save.
    let all = reads.read("/api/admin/history?limit=100", PAGE).await;
    assert_eq!(all["settings"].as_array().unwrap().len(), 3);
    // A cursor naming no record places no save rather than repeating them.
    let lost = reads.read("/api/admin/history?before=99", PAGE).await;
    assert_eq!(lost["settings"], json!([]));
}

#[tokio::test]
async fn a_run_log_pages_every_record_touching_one_run() {
    let reads = Reads::new().await;
    let moved = reads.move_kalos(6, "21:00").await;
    let reverted = reads
        .plan("/api/admin/history/revert", json!({"seqs": [moved]}), &[])
        .await["record"]["seq"]
        .as_u64()
        .unwrap();
    for (run, member) in [("r-kalos", "1004"), ("n-kalos", "1001")] {
        let v = reads.version().await;
        reads
            .ok(
                "POST",
                &format!("/api/admin/runs/{run}/rsvp"),
                json!({"member_id": member, "answer": "yes", "version": v}),
                "week.json#/$defs/RunResult",
            )
            .await;
    }
    let answered = reads.version().await - 1;
    assert_eq!((moved, reverted, answered), (2, 3, 4));

    // Newest first, paged like the full list; the seed created the run.
    let first = reads
        .read("/api/admin/history?run=r-kalos&limit=3", PAGE)
        .await;
    assert_eq!(
        (
            seqs(&first),
            first["total"].clone(),
            first["next_before"].clone()
        ),
        (vec![4, 3, 2], json!(4), json!(2))
    );
    assert_eq!(first["head"]["seq"], 5);
    let rest = reads
        .read("/api/admin/history?run=r-kalos&limit=3&before=2", PAGE)
        .await;
    assert_eq!(
        (seqs(&rest), rest["next_before"].clone()),
        (vec![1], Value::Null)
    );
    // Each record carries the run's row before and after, so the log can
    // show "field: before → after" without another read.
    let row = |record: &Value, table: &str| {
        record["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| {
                row["key"]["table"] == table
                    && (row["key"]["id"] == "r-kalos" || row["key"]["run_id"] == "r-kalos")
            })
            .cloned()
            .unwrap()
    };
    let undo = row(&first["records"][1], "runs");
    assert_eq!(first["records"][1]["refs"][0]["seq"], moved);
    assert_eq!(undo["before"]["datetime"], "2026-09-30T13:00:00+00:00");
    assert_eq!(undo["after"]["datetime"], "2026-09-29T14:00:00+00:00");
    let answer = row(&first["records"][0], "rsvps");
    assert_eq!(
        (answer["before"].clone(), answer["after"]["state"].clone()),
        (Value::Null, json!("yes"))
    );
    assert_eq!(
        first["records"][0]["actor"],
        json!({"kind": "admin", "id": "token"})
    );

    for bad in [
        "run=nope",
        "run=",
        "run=r%20kalos",
        "run=r/kalos",
        "run=r-kalos&run=r-kalos",
        "run=r-kalos&week=2026-09-24",
        "run=r-kalos&actor=admin:token",
    ] {
        assert_eq!(
            reads.status_of(&format!("/api/admin/history?{bad}")).await,
            (422, "invalid_query".into()),
            "{bad}"
        );
    }

    // A run removed behind the API (no writer removes runs) keeps its log.
    let gone = "/api/admin/history?run=n-star";
    assert_eq!(seqs(&reads.read(gone, PAGE).await), [1]);
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&reads.db_path)
        .connect()
        .await
        .unwrap();
    sqlx::query("DELETE FROM runs WHERE id = 'n-star'")
        .execute(&mut conn)
        .await
        .unwrap();
    sqlx::Connection::close(conn).await.unwrap();
    assert!(
        reads
            .store
            .load(&kanade::domain::scheduler::Scope::Run("n-star".into()))
            .await
            .unwrap()
            .runs
            .is_empty()
    );
    let page = reads.read(gone, PAGE).await;
    assert_eq!((seqs(&page), page["total"].clone()), (vec![1], json!(1)));
}

#[tokio::test]
async fn checkpoints_report_the_verified_chain() {
    let reads = Reads::new().await;
    reads.move_kalos(6, "21:00").await;
    let checkpoints = reads
        .read(
            "/api/admin/history/checkpoints",
            "history.json#/$defs/Checkpoints",
        )
        .await;
    assert_eq!(checkpoints["verified"]["ok"], true);
    assert_eq!(checkpoints["verified"]["checked"], 3, "genesis, seed, move");
    assert_eq!(checkpoints["verified"]["head"]["seq"], 2);
    assert_eq!(checkpoints["backup_dir_configured"], true);
    assert_eq!(checkpoints["backups"], json!([]));
}

const CHECKPOINTS: &str = "history.json#/$defs/Checkpoints";

/// A hand-written snapshot and manifest, as an older build or a damaged
/// directory would leave them.
fn write_backup(dir: &std::path::Path, file: &str, manifest: &str) {
    std::fs::write(dir.join(file), b"snapshot").unwrap();
    std::fs::write(dir.join(format!("{file}.manifest.json")), manifest).unwrap();
}

fn manifest(seq: u64, hash: &str, schema: i64, created_at: Option<&str>) -> String {
    let mut value = json!({
        "format": "kanade.backup.v1",
        "history_head": {"seq": seq, "hash": hash},
        "revision": 7,
        "schema_version": schema,
    });
    if let Some(at) = created_at {
        value["created_at"] = json!(at);
    }
    value.to_string()
}

#[tokio::test]
async fn checkpoints_list_backups_newest_first_with_their_anchor() {
    let reads = Reads::new().await;
    let dir = reads.backup_dir.clone().expect("configured");
    let seed = reads.store.history_head().await.unwrap();
    let schema = reads.store.schema_version().await.unwrap();
    // A real backup: its head is the current head.
    reads
        .store
        .backup(&dir.join("kanade-real.sqlite"))
        .await
        .unwrap();
    reads.move_kalos(6, "21:00").await;
    write_backup(
        &dir,
        "kanade-forked.sqlite",
        &manifest(
            seed.seq,
            &"0".repeat(64),
            schema,
            Some("2026-09-02T00:00:00+00:00"),
        ),
    );
    write_backup(
        &dir,
        "kanade-old.sqlite",
        &manifest(
            seed.seq,
            &seed.hash,
            schema - 1,
            Some("2026-09-01T00:00:00Z"),
        ),
    );
    // No created_at (an older manifest): the snapshot's mtime stands in.
    write_backup(
        &dir,
        "kanade-legacy.sqlite",
        &manifest(seed.seq, &seed.hash, schema, None),
    );
    let legacy = std::fs::File::options()
        .write(true)
        .open(dir.join("kanade-legacy.sqlite"))
        .unwrap();
    legacy
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_785_542_400))
        .unwrap();
    drop(legacy);
    // Skipped: unreadable, foreign format, snapshot gone; ignored: a tarball,
    // a staging directory, a manifest-less snapshot.
    write_backup(&dir, "kanade-bad.sqlite", "not json");
    write_backup(&dir, "kanade-foreign.sqlite", r#"{"format":"other.v9"}"#);
    std::fs::write(
        dir.join("kanade-gone.sqlite.manifest.json"),
        manifest(seed.seq, &seed.hash, schema, Some("2026-09-03T00:00:00Z")),
    )
    .unwrap();
    std::fs::write(dir.join("kanade_v5_data-pre-abc.tar.gz"), b"tar").unwrap();
    std::fs::create_dir(dir.join(".kanade-x.sqlite.partial-1")).unwrap();
    std::fs::write(dir.join("kanade-bare.sqlite"), b"snapshot").unwrap();
    // Symlinks are never followed: neither a linked manifest nor a linked
    // snapshot is listed, even when the target is a valid one.
    let outside = dir.parent().unwrap().join("outside");
    std::fs::create_dir(&outside).unwrap();
    let valid = manifest(seed.seq, &seed.hash, schema, Some("2026-09-04T00:00:00Z"));
    std::fs::write(outside.join("m.json"), &valid).unwrap();
    std::fs::write(outside.join("s.sqlite"), b"snapshot").unwrap();
    std::fs::write(dir.join("kanade-linked-manifest.sqlite"), b"snapshot").unwrap();
    std::os::unix::fs::symlink(
        outside.join("m.json"),
        dir.join("kanade-linked-manifest.sqlite.manifest.json"),
    )
    .unwrap();
    std::fs::write(
        dir.join("kanade-linked-snapshot.sqlite.manifest.json"),
        &valid,
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.join("s.sqlite"),
        dir.join("kanade-linked-snapshot.sqlite"),
    )
    .unwrap();

    let checkpoints = reads
        .read("/api/admin/history/checkpoints", CHECKPOINTS)
        .await;
    assert_eq!(checkpoints["verified"]["ok"], true);
    let backups = checkpoints["backups"].as_array().unwrap();
    let summary: Vec<(&str, &str, bool)> = backups
        .iter()
        .map(|backup| {
            (
                backup["file"].as_str().unwrap(),
                backup["anchor"].as_str().unwrap(),
                backup["anchored"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("kanade-real.sqlite", "matches", true),
            ("kanade-forked.sqlite", "mismatch", false),
            ("kanade-old.sqlite", "older_schema", true),
            ("kanade-legacy.sqlite", "matches", true),
        ]
    );
    let real = &backups[0];
    assert_eq!(real["history_head"]["seq"], seed.seq);
    assert_eq!(real["history_head"]["hash"], seed.hash.as_str());
    assert_eq!(real["schema_version"], schema);
    assert_eq!(real["format"], "kanade.backup.v1");
    assert!(real["created_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(backups[2]["created_at"], "2026-09-01T00:00:00Z");
    assert_eq!(backups[3]["created_at"], "2026-08-01T00:00:00Z");

    // Each read looks again: a removed snapshot drops out.
    std::fs::remove_file(dir.join("kanade-forked.sqlite")).unwrap();
    let again = reads
        .read("/api/admin/history/checkpoints", CHECKPOINTS)
        .await;
    assert_eq!(again["backups"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn checkpoints_say_when_no_backup_dir_is_configured() {
    let reads = Reads::without_backup_dir().await;
    let checkpoints = reads
        .read("/api/admin/history/checkpoints", CHECKPOINTS)
        .await;
    assert_eq!(checkpoints["backup_dir_configured"], false);
    assert_eq!(checkpoints["backups"], json!([]));
    assert_eq!(checkpoints["verified"]["ok"], true);
}

#[tokio::test]
async fn revert_previews_then_applies_once() {
    let reads = Reads::new().await;
    let before = reads.kalos().await;
    let moved = reads.move_kalos(6, "21:00").await;
    let v = reads.version().await;

    let preview = reads
        .plan(
            "/api/admin/history/revert",
            json!({"seqs": [moved], "preview": true, "request_id": "ignored-on-preview"}),
            &[],
        )
        .await;
    assert_eq!(preview["outcome"], "preview");
    assert_eq!(preview["reverts"], json!([moved]));
    assert_eq!(preview["record"], Value::Null);
    assert!(!schedule_rows(&preview).is_empty());
    assert_eq!(reads.version().await, v, "a preview writes nothing");
    assert_eq!(reads.kalos().await["day"], 6);

    let key = [("Idempotency-Key", "revert-1")];
    let applied = reads
        .plan("/api/admin/history/revert", json!({"seqs": [moved]}), &key)
        .await;
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(schedule_rows(&applied), schedule_rows(&preview));
    let record = &applied["record"];
    assert_eq!(record["seq"], v + 1);
    assert_eq!(record["surface"], "rollback");
    assert_eq!(record["actor"], json!({"kind": "admin", "id": "token"}));
    assert_eq!(record["request_id"], "revert-1");
    assert_eq!(record["refs"][0]["seq"], moved);
    assert_eq!(applied["rows"], record["rows"]);
    let back = reads.kalos().await;
    assert_eq!(
        (back["day"].clone(), back["time"].clone()),
        (before["day"].clone(), before["time"].clone())
    );
    let stored = reads.store.load_change(v + 1).await.unwrap().unwrap();
    assert_eq!(stored.origin.surface, Surface::Rollback);

    // A retry answers the recorded rollback, whichever way the id is sent.
    let again = reads
        .plan(
            "/api/admin/history/revert",
            json!({"seqs": [moved], "request_id": "revert-1"}),
            &[],
        )
        .await;
    assert_eq!(again["outcome"], "applied");
    assert_eq!(again["record"], applied["record"]);
    assert_eq!(again["reverts"], json!([moved]));
    assert_eq!(reads.version().await, v + 1, "applied once");

    let reused = reads
        .call(
            "POST",
            "/api/admin/history/revert",
            json!({"seqs": [1]}),
            &key,
        )
        .await;
    assert_eq!(
        (reused.status, reused.api_error()),
        (422, "idempotency_mismatch".into())
    );
    let split = reads
        .call(
            "POST",
            "/api/admin/history/revert",
            json!({"seqs": [moved], "request_id": "other"}),
            &key,
        )
        .await;
    assert_eq!(
        (split.status, split.api_error()),
        (400, "invalid_idempotency_key".into())
    );
    for (body, status, code) in [
        (json!({"seqs": []}), 422, "invalid"),
        (json!({"seqs": [0]}), 422, "invalid"),
        (json!({"seqs": [99]}), 422, "invalid"),
        (
            json!({"seqs": [1], "request_id": "no spaces"}),
            400,
            "invalid_idempotency_key",
        ),
        (json!({"seqs": [1], "colour": 1}), 400, "invalid_body"),
    ] {
        assert_eq!(
            reads
                .refused("POST", "/api/admin/history/revert", body.clone())
                .await,
            (status, code.into()),
            "{body}"
        );
    }
    assert_eq!(reads.version().await, v + 1);
}

#[tokio::test]
async fn strict_reverts_report_conflicts_and_force_overrides_them() {
    let reads = Reads::new().await;
    let first = reads.move_kalos(6, "21:00").await;
    reads.move_kalos(4, "20:00").await;
    let v = reads.version().await;
    for preview in [true, false] {
        let plan = reads
            .plan(
                "/api/admin/history/revert",
                json!({"seqs": [first], "preview": preview}),
                &[],
            )
            .await;
        assert_eq!(plan["outcome"], "conflicts");
        assert_eq!(plan["reverts"], json!([first]));
        assert_eq!(plan["conflicts"][0]["seq"], first);
        assert_eq!(
            plan["conflicts"][0]["key"],
            json!({"table": "runs", "id": "r-kalos"})
        );
        assert_eq!(reads.version().await, v, "strict conflicts write nothing");
    }
    let forced = reads
        .plan(
            "/api/admin/history/revert",
            json!({"seqs": [first], "force": true, "preview": true}),
            &[],
        )
        .await;
    assert_eq!(forced["outcome"], "preview");
    assert!(!forced["conflicts"].as_array().unwrap().is_empty());
    assert_eq!(reads.version().await, v);
    let applied = reads
        .plan(
            "/api/admin/history/revert",
            json!({"seqs": [first], "force": true}),
            &[],
        )
        .await;
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(reads.version().await, v + 1);
    assert_eq!(reads.kalos().await["day"], 5, "back to the seeded Tuesday");
}

#[tokio::test]
async fn week_restores_and_actor_reverts_preview_then_apply() {
    let reads = Reads::new().await;
    let seed = reads.read("/api/admin/history/1", RECORD).await;
    let revision = seed["revision"].as_u64().unwrap();
    reads.move_kalos(6, "21:00").await;
    reads.move_kalos(4, "20:00").await;
    let v = reads.version().await;

    let body =
        |preview: bool| json!({"week": "2026-09-24", "revision": revision, "preview": preview});
    let preview = reads
        .plan("/api/admin/history/restore-week", body(true), &[])
        .await;
    assert_eq!(preview["outcome"], "preview");
    assert_eq!(preview["reverts"], json!([3, 2]));
    assert_eq!(reads.version().await, v);
    let applied = reads
        .plan("/api/admin/history/restore-week", body(false), &[])
        .await;
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(schedule_rows(&applied), schedule_rows(&preview));
    assert_eq!(reads.kalos().await["day"], 5);
    assert_eq!(
        reads
            .refused(
                "POST",
                "/api/admin/history/restore-week",
                json!({"week": "2026-09-25", "revision": revision}),
            )
            .await,
        (422, "invalid".into())
    );
    // Nothing in next week changed after the seed.
    let quiet = reads
        .plan(
            "/api/admin/history/restore-week",
            json!({"week": "2026-09-30T16:00:00+00:00", "revision": revision + 100, "preview": true}),
            &[],
        )
        .await;
    assert_eq!(quiet["outcome"], "unchanged");

    // Everything admin:token did today, including the restore it ran.
    let v = reads.version().await;
    reads.move_kalos(3, "19:00").await;
    let actor = |since: &str, preview: bool| json!({"actor": "admin:token", "since": since, "preview": preview});
    let preview = reads
        .plan(
            "/api/admin/history/revert-actor",
            actor("2026-09-29", true),
            &[],
        )
        .await;
    assert_eq!(preview["outcome"], "preview");
    assert_eq!(preview["reverts"], json!([v + 1, v, 3, 2]));
    assert_eq!(reads.version().await, v + 1);
    let later = reads
        .plan(
            "/api/admin/history/revert-actor",
            actor("2026-09-29T13:00", true),
            &[],
        )
        .await;
    assert_eq!(later["outcome"], "unchanged", "nothing after 13:00 KL");
    let applied = reads
        .plan(
            "/api/admin/history/revert-actor",
            json!({"actor": "admin:token", "since": "2026-09-29T00:00:00", "force": true}),
            &[("Idempotency-Key", "spam-1")],
        )
        .await;
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(applied["record"]["seq"], v + 2);
    for (body, what) in [
        (actor("2026-09-29T00:00:00Z", false), "offset"),
        (actor("29/09/2026", false), "shape"),
        (json!({"actor": "robot:1", "since": "2026-09-29"}), "actor"),
        (json!({"actor": "admin:token"}), "since missing"),
    ] {
        let (status, _) = reads
            .refused("POST", "/api/admin/history/revert-actor", body)
            .await;
        assert!(matches!(status, 400 | 422), "{what}: {status}");
    }
}

#[tokio::test]
async fn rollbacks_need_csrf_even_to_preview() {
    let reads = Reads::new().await;
    let reply = crate::support::send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        "/api/admin/history/revert",
        &[
            ("Cookie", reads.cookie.as_str()),
            ("Origin", "https://kanade.test"),
        ],
        Some(&json!({"seqs": [1], "preview": true}).to_string()),
    )
    .await;
    assert_eq!((reply.status, reply.api_error()), (403, "csrf".into()));
}

/// Seqs past i64::MAX (what SQLite stores) name no record: contract codes,
/// never 503.
#[tokio::test]
async fn seqs_beyond_the_store_range_get_contract_codes() {
    let reads = Reads::new().await;
    assert_eq!(
        reads
            .status_of("/api/admin/history?before=18446744073709551615")
            .await,
        (422, "invalid_query".into())
    );
    assert_eq!(
        reads
            .status_of("/api/admin/history/9223372036854775808")
            .await,
        (404, "not_found".into())
    );
    assert_eq!(
        reads
            .refused(
                "POST",
                "/api/admin/history/revert",
                json!({"seqs": [9223372036854775808_u64]}),
            )
            .await,
        (422, "invalid".into())
    );
    // i64::MAX itself is in range: an older-than cursor, and a missing record.
    let page = reads
        .read("/api/admin/history?before=9223372036854775807", PAGE)
        .await;
    assert_eq!(seqs(&page), [1]);
}

/// Strict conflicts list every selected record in `reverts`, not only the
/// conflicting ones. A consistent history cannot make a week restore
/// conflict (every later change to the week is selected), so this uses the
/// actor revert, which shares the path: Bob's later answer conflicts with one
/// of the admin's two records only.
#[tokio::test]
async fn strict_conflicts_list_all_selected_records() {
    let reads = Reads::new().await;
    let moved = reads.move_kalos(6, "21:00").await;
    let v = reads.version().await;
    reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1004", "answer": "yes", "version": v}),
            "week.json#/$defs/RunResult",
        )
        .await;
    let answered = moved + 1;
    // Someone else changes that answer afterwards (not the admin's actor).
    reads
        .store
        .commit(
            reads
                .store
                .load(&kanade::domain::scheduler::Scope::All)
                .await
                .unwrap()
                .revision,
            kanade::domain::schedule::ChangeSet {
                changes: vec![kanade::domain::schedule::Change::DeleteRsvp {
                    run_id: "r-kalos".into(),
                    user_id: "1004".into(),
                }],
            },
            kanade::domain::history::ChangeMeta {
                origin: kanade::domain::history::Origin::new(
                    kanade::domain::history::Actor::member("1004"),
                    Surface::Discord,
                ),
                at: chrono::DateTime::UNIX_EPOCH + chrono::TimeDelta::days(20_725),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            },
        )
        .await
        .unwrap();
    let head = reads.version().await;
    let plan = reads
        .plan(
            "/api/admin/history/revert-actor",
            json!({"actor": "admin:token", "since": "2026-09-01"}),
            &[],
        )
        .await;
    assert_eq!(plan["outcome"], "conflicts");
    assert_eq!(plan["reverts"], json!([answered, moved]));
    let conflicting: Vec<_> = plan["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|conflict| conflict["seq"].clone())
        .collect();
    assert!(
        conflicting.iter().all(|seq| *seq == answered),
        "{conflicting:?}"
    );
    assert_eq!(reads.version().await, head, "nothing written");
}

/// An applied rollback answers the committed record's rows, RSVP rows
/// included, not the plan.
#[tokio::test]
async fn forced_week_restore_answers_the_record_rows() {
    let reads = Reads::new().await;
    let revision = reads.read("/api/admin/history/1", RECORD).await["revision"]
        .as_u64()
        .unwrap();
    for (member, answer) in [("1004", "yes"), ("1001", "clear")] {
        let v = reads.version().await;
        reads
            .ok(
                "POST",
                "/api/admin/runs/r-kalos/rsvp",
                json!({"member_id": member, "answer": answer, "version": v}),
                "week.json#/$defs/RunResult",
            )
            .await;
    }
    reads.move_kalos(6, "21:00").await;
    let applied = reads
        .plan(
            "/api/admin/history/restore-week",
            json!({"week": "2026-09-24", "revision": revision, "force": true}),
            &[],
        )
        .await;
    assert_eq!(applied["outcome"], "applied");
    assert_eq!(applied["rows"], applied["record"]["rows"]);
    let tables: Vec<_> = applied["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["key"]["table"].as_str().unwrap().to_owned())
        .collect();
    assert!(tables.iter().any(|table| table == "rsvps"), "{tables:?}");
    assert!(tables.iter().any(|table| table == "runs"), "{tables:?}");
}
