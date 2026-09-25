//! A5 history against the seeded store of `reads.rs` (one seed record by
//! `admin:seed`; the pinned clock is Tue 29 Sep 12:00 KL; this boss week
//! starts Thu 24 Sep 00:00 KL = 2026-09-23T16:00:00Z). Every response is
//! validated against the (A5-extended) schemas.

use kanade::domain::history::{ChangeHistory, Surface};
use kanade::domain::scheduler::ScheduleStore;
use serde_json::{Value, json};

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

#[tokio::test]
async fn blame_names_domain_fields_with_current_values() {
    let reads = Reads::new().await;
    reads.move_kalos(6, "21:00").await;
    let v = reads.version().await;
    reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1004", "answer": "yes", "version": v}),
            "week.json#/$defs/RunResult",
        )
        .await;
    let blame = reads
        .read(
            "/api/admin/runs/r-kalos/blame",
            "history.json#/$defs/BlameEntries",
        )
        .await;
    let entry = |field: &str| {
        blame
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["field"] == field)
            .unwrap_or_else(|| panic!("{field}: {blame:#}"))
            .clone()
    };
    let slot = entry("slot");
    assert_eq!(slot["seq"], 2);
    assert_eq!(slot["actor"], json!({"kind": "admin", "id": "token"}));
    // Day 6 of the week from Thu 24 Sep: Wed 30 Sep, 21:00 KL.
    assert_eq!(slot["value"]["datetime"], "2026-09-30T13:00:00+00:00");
    let answer = entry("rsvp:1004");
    assert_eq!(answer["seq"], 3);
    assert_eq!(answer["value"]["state"], "yes");
    assert_eq!(answer["value"]["source"], "chat");
    assert_eq!(entry("participants")["seq"], 1, "set by the seed");
    assert_eq!(
        reads.status_of("/api/admin/runs/nope/blame").await,
        (404, "not_found".into())
    );
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
    assert_eq!(checkpoints["backups"], json!([]));
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
