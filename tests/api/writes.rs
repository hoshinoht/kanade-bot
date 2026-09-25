//! A4 admin mutations against the seeded store of `reads.rs` (pinned clock
//! Tue 29 Sep 12:00 KL; `r-kalos` is Tue 22:00 this week, amended off its
//! weekly timing `f-kalos`). Every response is validated against A0.

use kanade::domain::history::{BlameTarget, ChangeHistory, Surface, changed_fields};
use serde_json::{Value, json};

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, send},
};

const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const RUN_RESULT: &str = "week.json#/$defs/RunResult";
const ERROR: &str = "error.json#/$defs/ApiError";

impl Reads {
    async fn call(&self, method: &str, path: &str, body: Value, extra: &[(&str, &str)]) -> Reply {
        let mut headers = vec![
            ("Cookie", self.cookie.as_str()),
            ORIGIN,
            ("X-Kanade-CSRF", self.csrf.as_str()),
        ];
        headers.extend_from_slice(extra);
        send(
            self.admin,
            method,
            ADMIN_HOST,
            path,
            &headers,
            Some(&body.to_string()),
        )
        .await
    }

    /// 2xx validated against `target`, anything else against `ApiError`.
    async fn ok(&self, method: &str, path: &str, body: Value, target: &str) -> Value {
        let reply = self.call(method, path, body, &[]).await;
        assert!(
            (200..300).contains(&reply.status),
            "{method} {path}: {}",
            reply.text()
        );
        let value = reply.json();
        assert_valid(target, path, &value);
        value
    }

    async fn refused(&self, method: &str, path: &str, body: Value) -> (u16, String) {
        let reply = self.call(method, path, body, &[]).await;
        assert!(reply.status >= 400, "{method} {path}: {}", reply.text());
        assert_valid(ERROR, path, &reply.json());
        (reply.status, reply.api_error())
    }

    async fn version(&self) -> u64 {
        self.store.history_head().await.unwrap().seq
    }
}

#[tokio::test]
async fn moves_are_attributed_to_the_session_and_csrf_is_required() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let body = json!({"day": 6, "time": "21:00", "version": v});
    let reply = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            body.clone(),
            "week.json#/$defs/MoveResult",
        )
        .await;
    assert_eq!(reply["previous"], json!({"day": 5, "time": "22:00"}));
    assert_eq!(
        (reply["run"]["day"].clone(), reply["run"]["time"].clone()),
        (6.into(), "21:00".into())
    );
    assert_eq!(reply["version"], v + 1);
    let record = reads.store.load_change(v + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.actor.id(), "token");
    assert_eq!(record.origin.surface, Surface::AdminPortal);

    // No CSRF token, or no session: refused before anything is written.
    let no_csrf = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        "/api/admin/runs/r-kalos/move",
        &[("Cookie", reads.cookie.as_str()), ORIGIN],
        Some(&body.to_string()),
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    let anonymous = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        "/api/admin/runs/r-kalos/move",
        &[ORIGIN],
        Some(&body.to_string()),
    )
    .await;
    assert_eq!(anonymous.status, 401);
    assert_eq!(reads.version().await, v + 1);

    // The CLI's bearer needs no CSRF token and is recorded as the CLI.
    let bearer = format!("Bearer {}", "break-glass-token-with-at-least-32-bytes!");
    let reply = send(
        reads.admin,
        "PATCH",
        ADMIN_HOST,
        "/api/admin/runs/r-kalos/status",
        &[("Authorization", bearer.as_str())],
        Some(&json!({"status": "confirmed", "version": v + 1}).to_string()),
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let record = reads.store.load_change(v + 2).await.unwrap().unwrap();
    assert_eq!(record.origin.surface, Surface::Cli);
}

/// Every pair of run edits made from the same (head-first) week version:
/// the second is refused exactly when the first changed its field (field-level
/// merge, no false 409; a same-field pair always conflicts), and a refused
/// second edit never overwrites the first (no lost update).
#[tokio::test]
async fn same_version_edits_merge_by_field_and_never_lose_an_update() {
    type Edit = (&'static str, &'static str, &'static str, fn(u64) -> Value);
    let edits: [Edit; 5] = [
        (
            "slot",
            "POST",
            "move",
            |v| json!({"day": 6, "time": "20:00", "version": v}),
        ),
        (
            "status",
            "PATCH",
            "status",
            |v| json!({"status": "confirmed", "version": v}),
        ),
        (
            "participants",
            "PATCH",
            "participants",
            |v| json!({"remove": "1004", "version": v}),
        ),
        (
            "rsvp:1001",
            "POST",
            "rsvp",
            |v| json!({"member_id": "1001", "answer": "no", "version": v}),
        ),
        (
            "rsvp:1002",
            "POST",
            "rsvp",
            |v| json!({"member_id": "1002", "answer": "yes", "version": v}),
        ),
    ];
    for (first_field, first_method, first_path, first_body) in edits {
        for (second_field, second_method, second_path, second_body) in edits {
            let reads = Reads::new().await;
            // What the PWA holds: the week's version, read before its data.
            let v = reads.read("/api/admin/week", "week.json#/$defs/Week").await["version"]
                .as_u64()
                .unwrap();
            let first = format!("/api/admin/runs/r-kalos/{first_path}");
            let second = format!("/api/admin/runs/r-kalos/{second_path}");
            let target = |path: &str| {
                if path == "move" {
                    "week.json#/$defs/MoveResult"
                } else {
                    RUN_RESULT
                }
            };
            let after_first = reads
                .ok(first_method, &first, first_body(v), target(first_path))
                .await;
            let second_reply = reads
                .call(second_method, &second, second_body(v), &[])
                .await;
            let pair = format!("{first_field} then {second_field}");
            // What the first edit really changed (a roster or answer change may
            // also re-derive the status): exactly those fields may 409.
            let changed = changed_fields(&reads.store.load_change(v + 1).await.unwrap().unwrap());
            let conflicts =
                changed.contains(&(BlameTarget::Run("r-kalos".into()), second_field.to_owned()));
            assert!(
                first_field != second_field || conflicts,
                "{pair}: an edit changes its own field"
            );
            if conflicts {
                assert_eq!(
                    (second_reply.status, second_reply.api_error()),
                    (409, "stale".into()),
                    "{pair}"
                );
                let run = reads.read("/api/admin/week", "week.json#/$defs/Week").await["runs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|run| run["id"] == "r-kalos")
                    .unwrap()
                    .clone();
                let kept = &after_first["run"];
                for key in ["day", "time", "status", "participants"] {
                    assert_eq!(run[key], kept[key], "{pair}: first edit survives ({key})");
                }
            } else {
                assert_eq!(second_reply.status, 200, "{pair}: {}", second_reply.text());
                assert_valid(target(second_path), &pair, &second_reply.json());
            }
        }
    }
}

#[tokio::test]
async fn idempotency_keys_replay_the_first_result_and_refuse_reuse() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let body = json!({"day": 6, "time": "21:00", "version": v});
    let key = [("Idempotency-Key", "move-1")];
    let first = reads
        .call("POST", "/api/admin/runs/r-kalos/move", body.clone(), &key)
        .await;
    assert_eq!(first.status, 200);
    // A retry after the slot moved (by itself) replays instead of 409.
    let again = reads
        .call("POST", "/api/admin/runs/r-kalos/move", body, &key)
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(again.json()["run"], first.json()["run"]);
    assert_eq!(reads.version().await, v + 1, "applied once");
    // A fresh key from the same stale version is a real conflict, not a replay.
    let stale = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 4, "time": "21:00", "version": v}),
            &[("Idempotency-Key", "move-2")],
        )
        .await;
    assert_eq!((stale.status, stale.api_error()), (409, "stale".into()));

    let other = json!({"day": 4, "time": "21:00", "version": v});
    let reused = reads
        .call("POST", "/api/admin/runs/r-kalos/move", other, &key)
        .await;
    assert_eq!(
        (reused.status, reused.api_error()),
        (422, "idempotency_mismatch".into())
    );
    let bad = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 1, "time": "21:00", "version": v}),
            &[("Idempotency-Key", "no spaces")],
        )
        .await;
    assert_eq!(
        (bad.status, bad.api_error()),
        (400, "invalid_idempotency_key".into())
    );
}

#[tokio::test]
async fn explicit_expectations_and_admin_overrides() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 6, "time": "21:00", "version": v}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    let head = reads.store.history_head().await.unwrap();
    // Declaring the stale `seen` is a conflict; overriding the change it names applies.
    let stale =
        json!({"day": 4, "time": "21:00", "version": v, "expect": [{"field": "slot", "seen": v}]});
    assert_eq!(
        reads
            .refused("POST", "/api/admin/runs/r-kalos/move", stale)
            .await,
        (409, "stale".into())
    );
    let unknown = json!({"day": 4, "time": "21:00", "version": v, "expect": [{"field": "colour", "seen": null}]});
    assert_eq!(
        reads
            .refused("POST", "/api/admin/runs/r-kalos/move", unknown)
            .await,
        (422, "unknown_field".into())
    );
    let forced = json!({
        "day": 4, "time": "21:00", "version": v,
        "expect": [{"field": "slot", "seen": head.seq}],
        "override": [{"seq": head.seq, "hash": head.hash}],
    });
    let reply = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            forced,
            "week.json#/$defs/MoveResult",
        )
        .await;
    assert_eq!(reply["run"]["day"], 4);
}

#[tokio::test]
async fn run_edits_refuse_with_the_table_codes() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let cases: [(&str, &str, Value, u16, &str); 10] = [
        (
            "PATCH",
            "/api/admin/runs/r-kalos/status",
            json!({"status": "sideways", "version": v}),
            422,
            "invalid",
        ),
        (
            "PATCH",
            "/api/admin/runs/r-kalos/status",
            json!({"status": "at_risk", "version": v}),
            422,
            "invalid",
        ),
        (
            "PATCH",
            "/api/admin/runs/nope/status",
            json!({"status": "done", "version": v}),
            404,
            "not_found",
        ),
        (
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 7, "time": "21:00", "version": v}),
            422,
            "invalid",
        ),
        (
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 1, "time": "9pm", "version": v}),
            422,
            "invalid",
        ),
        (
            "POST",
            "/api/admin/runs/r-star/move",
            json!({"day": 1, "time": "21:00", "version": v}),
            422,
            "invalid",
        ),
        (
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1006", "answer": "yes", "version": v}),
            422,
            "not_on_run",
        ),
        (
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"add": "1006", "version": v}),
            422,
            "invalid",
        ),
        (
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"version": v}),
            422,
            "invalid",
        ),
        (
            "PATCH",
            "/api/admin/runs/r-kalos/status",
            json!({"status": "done", "version": v, "colour": 1}),
            400,
            "invalid_body",
        ),
    ];
    for (method, path, body, status, code) in cases {
        assert_eq!(
            reads.refused(method, path, body.clone()).await,
            (status, code.into()),
            "{method} {path} {body}"
        );
    }
    assert_eq!(reads.version().await, v, "nothing was written");
}

#[tokio::test]
async fn rsvp_participants_reset_and_ping() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let answer = |run: &Value, id: &str| {
        run["run"]["participants"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .map(|p| p["answer"].clone())
    };
    let run = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1004", "answer": "yes", "version": v}),
            RUN_RESULT,
        )
        .await;
    assert_eq!(answer(&run, "1004"), Some("yes".into()));
    let run = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1001", "answer": "clear", "version": v}),
            RUN_RESULT,
        )
        .await;
    assert_eq!(answer(&run, "1001"), Some("waiting".into()));

    let run = reads
        .ok(
            "PATCH",
            "/api/admin/runs/r-kalos/participants",
            json!({"remove": "1004", "version": v}),
            RUN_RESULT,
        )
        .await;
    assert_eq!(answer(&run, "1004"), None);

    let run = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/reset",
            json!({"version": reads.version().await}),
            RUN_RESULT,
        )
        .await;
    assert_eq!(run["run"]["amended"], false);
    assert_eq!(run["run"]["roster_change"], Value::Null);

    let ping = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/ping",
            json!({}),
            "common.json#/$defs/Message",
        )
        .await;
    assert!(ping["message"].as_str().unwrap().contains("not posted"));
    assert_eq!(
        reads
            .refused("POST", "/api/admin/runs/nope/ping", json!({}))
            .await,
        (404, "not_found".into())
    );
}

#[tokio::test]
async fn weekly_timings_create_edit_with_decisions_and_retire() {
    let reads = Reads::new().await;
    let create = json!({
        "weekday": 3, "time": "21:00", "bosses": "hstar", "participants": ["1001", "1004"],
        "channel_id": "kalos-four", "note": " "
    });
    let key = [("Idempotency-Key", "new-timing")];
    let created = reads
        .call("POST", "/api/admin/fixed", create.clone(), &key)
        .await;
    assert_eq!(created.status, 201, "{}", created.text());
    let row = created.json();
    assert_valid("fixed.json#/$defs/FixedRow", "create", &row);
    assert_eq!(row["weekday_name"], "Thursday");
    assert_eq!(row["bosses"][0]["token"], "HMaleficStar");
    assert_eq!(row["note"], Value::Null);
    assert_eq!(
        row["runs"][0]["week"], "next",
        "this week's Thursday has passed"
    );
    let replay = reads
        .call("POST", "/api/admin/fixed", create.clone(), &key)
        .await;
    assert_eq!(replay.status, 201);
    assert_eq!(replay.json()["id"], row["id"]);
    let rows = reads
        .read("/api/admin/fixed", "fixed.json#/$defs/FixedRows")
        .await;
    assert_eq!(rows.as_array().unwrap().len(), 2, "created once");

    let mut unwatched = create.clone();
    unwatched["channel_id"] = "star".into();
    assert_eq!(
        reads.refused("POST", "/api/admin/fixed", unwatched).await.0,
        422
    );
    let mut unknown_boss = create.clone();
    unknown_boss["bosses"] = "hwhatever".into();
    assert_eq!(
        reads
            .refused("POST", "/api/admin/fixed", unknown_boss)
            .await,
        (422, "invalid".into())
    );

    // f-kalos: Tue 22:00. Move this week's run off it; moving the timing then needs a decision.
    let v = reads.version().await;
    reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 5, "time": "23:00", "version": v}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    let edit = json!({
        "weekday": 1, "time": "21:30", "bosses": "xkalos", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": "bring pots"
    });
    assert_eq!(
        reads
            .refused("PATCH", "/api/admin/fixed/f-kalos", edit.clone())
            .await,
        (422, "choices_required".into())
    );
    let mut kept = edit.clone();
    kept["decisions"] = json!({"r-kalos": "keep"});
    let row = reads
        .ok(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            kept,
            "fixed.json#/$defs/FixedRow",
        )
        .await;
    assert_eq!(row["time"], "21:30");
    let times: Vec<_> = row["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["run_id"].clone(), r["time"].clone()))
        .collect();
    assert_eq!(
        times,
        [
            ("r-kalos".into(), "23:00".into()),
            ("n-kalos".into(), "21:30".into())
        ]
    );
    let mut wrong = edit.clone();
    wrong["time"] = "20:00".into();
    wrong["decisions"] = json!({"n-kalos": "keep"});
    assert_eq!(
        reads
            .refused("PATCH", "/api/admin/fixed/f-kalos", wrong)
            .await
            .1,
        "choices_not_applicable"
    );

    let retired = reads
        .ok(
            "DELETE",
            "/api/admin/fixed/f-kalos",
            json!({}),
            "fixed.json#/$defs/FixedRetired",
        )
        .await;
    assert_eq!(
        retired["cancelled"], 3,
        "this, next and the materialised week after"
    );
    assert_eq!(
        reads
            .refused("DELETE", "/api/admin/fixed/f-kalos", json!({}))
            .await,
        (404, "not_found".into())
    );
    assert_eq!(
        reads
            .refused("PATCH", "/api/admin/fixed/f-kalos", edit)
            .await,
        (404, "not_found".into())
    );

    let valid = reads
        .ok(
            "POST",
            "/api/admin/validate/bosses",
            json!({"text": "xkalos, hstar"}),
            "fixed.json#/$defs/ValidateResult",
        )
        .await;
    assert_eq!(valid["bosses"].as_array().unwrap().len(), 2);
    assert_eq!(
        reads
            .refused(
                "POST",
                "/api/admin/validate/bosses",
                json!({"text": "kalos"})
            )
            .await
            .1,
        "invalid"
    );
}

#[tokio::test]
async fn member_edits_and_aliases() {
    let reads = Reads::new().await;
    let row = reads
        .ok(
            "PATCH",
            "/api/admin/members/1001",
            json!({"ping_level": "all", "persona": "default"}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(
        (row["ping_level"].clone(), row["persona"].clone()),
        ("all".into(), "default".into())
    );
    let row = reads
        .ok(
            "PATCH",
            "/api/admin/members/1001",
            json!({"persona": ""}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(row["persona"], Value::Null);
    assert_eq!(row["aliases"], json!(["ali"]), "untouched");

    let row = reads
        .ok(
            "POST",
            "/api/admin/members/1002/aliases",
            json!({"alias": " Bobby "}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(row["aliases"], json!(["bobby"]));
    for (path, body, status, code) in [
        (
            "/api/admin/members/1002/aliases",
            json!({"alias": "ali"}),
            422,
            "alias_taken",
        ),
        (
            "/api/admin/members/1002/aliases",
            json!({"alias": "two words"}),
            422,
            "invalid",
        ),
        (
            "/api/admin/members/1002",
            json!({"ping_level": "loud"}),
            422,
            "invalid",
        ),
        (
            "/api/admin/members/1002",
            json!({"persona": "pirate"}),
            422,
            "invalid",
        ),
        (
            "/api/admin/members/9999",
            json!({"ping_level": "all"}),
            404,
            "not_found",
        ),
    ] {
        let method = if path.ends_with("aliases") {
            "POST"
        } else {
            "PATCH"
        };
        assert_eq!(
            reads.refused(method, path, body).await,
            (status, code.into()),
            "{path}"
        );
    }
}
