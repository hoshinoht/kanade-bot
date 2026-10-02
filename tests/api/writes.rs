//! A4 admin mutations against the seeded store of `reads.rs` (pinned clock
//! Tue 29 Sep 12:00 KL; `r-kalos` is Tue 22:00 this week, amended off its
//! weekly timing `f-kalos`). Every response is validated against A0.

use kanade::domain::{
    history::{BlameTarget, ChangeHistory, ChangeMeta, Origin, Surface, changed_fields},
    members::MemberStore,
    notify::{DeclineNotice, DeclineNoticeStore, NoticeOutbox},
    schedule::{Change, ChangeSet, Rsvp, RsvpSource, RsvpState},
};
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
    pub(crate) async fn call(
        &self,
        method: &str,
        path: &str,
        body: Value,
        extra: &[(&str, &str)],
    ) -> Reply {
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
    pub(crate) async fn ok(&self, method: &str, path: &str, body: Value, target: &str) -> Value {
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

    pub(crate) async fn refused(&self, method: &str, path: &str, body: Value) -> (u16, String) {
        let reply = self.call(method, path, body, &[]).await;
        assert!(reply.status >= 400, "{method} {path}: {}", reply.text());
        assert_valid(ERROR, path, &reply.json());
        (reply.status, reply.api_error())
    }

    pub(crate) async fn version(&self) -> u64 {
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

#[tokio::test]
async fn planner_swap_is_atomic_replayable_and_revertible() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let before = reads
        .read("/api/admin/week?week=next", "week.json#/$defs/Week")
        .await;
    let run = |id: &str| {
        before["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["id"] == id)
            .unwrap()
            .clone()
    };
    let kalos = run("n-kalos");
    let own_time = run("n-star");
    let body = json!({"with": "n-star", "version": v});
    let key = [("Idempotency-Key", "planner-swap-1")];
    let swapped = reads
        .call("POST", "/api/admin/runs/n-kalos/swap", body.clone(), &key)
        .await;
    assert_eq!(swapped.status, 200, "{}", swapped.text());
    let swapped = swapped.json();
    assert_valid("week.json#/$defs/SwapResult", "swap", &swapped);
    assert_eq!(swapped["version"], v + 1);
    assert_eq!(swapped["runs"][0]["id"], "n-kalos");
    assert_eq!(swapped["runs"][1]["id"], "n-star");
    assert_eq!(swapped["runs"][0]["day"], own_time["day"]);
    assert_eq!(swapped["runs"][0]["time"], kalos["time"]);
    assert_eq!(swapped["runs"][1]["day"], kalos["day"]);
    assert_eq!(
        swapped["runs"][1]["time"], own_time["time"],
        "own-time runs keep their clock"
    );
    let record = reads.store.load_change(v + 1).await.unwrap().unwrap();
    let changed = changed_fields(&record);
    assert!(changed.contains(&(BlameTarget::Run("n-kalos".into()), "slot".into())));
    assert!(changed.contains(&(BlameTarget::Run("n-star".into()), "slot".into())));
    assert_eq!(
        reads.store.outbox_notices().await.unwrap().len(),
        2,
        "one move notice per run"
    );

    let replay = reads
        .call("POST", "/api/admin/runs/n-kalos/swap", body, &key)
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json(), swapped, "same response on replay");
    assert_eq!(reads.version().await, v + 1, "one record only");
    assert_eq!(reads.store.outbox_notices().await.unwrap().len(), 2);
    let changed_version = reads
        .call(
            "POST",
            "/api/admin/runs/n-kalos/swap",
            json!({"with": "n-star", "version": v + 1}),
            &key,
        )
        .await;
    assert_eq!(
        (changed_version.status, changed_version.api_error()),
        (422, "idempotency_mismatch".into())
    );
    assert_eq!(
        reads.version().await,
        v + 1,
        "changed-version reuse writes nothing"
    );

    reads
        .ok(
            "POST",
            "/api/admin/history/revert",
            json!({"seqs": [v + 1]}),
            "history.json#/$defs/RevertPlan",
        )
        .await;
    let restored = reads
        .read("/api/admin/week?week=next", "week.json#/$defs/Week")
        .await;
    for original in [kalos, own_time] {
        let current = restored["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["id"] == original["id"])
            .unwrap();
        assert_eq!(
            (current["day"].clone(), current["time"].clone()),
            (original["day"].clone(), original["time"].clone())
        );
    }
}

#[tokio::test]
async fn planner_swap_refusals_leave_both_runs_unchanged() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    for (path, body) in [
        (
            "/api/admin/runs/n-kalos/swap",
            json!({"with": "n-kalos", "version": v}),
        ),
        (
            "/api/admin/runs/n-kalos/swap",
            json!({"with": "r-kalos", "version": v}),
        ),
        (
            "/api/admin/runs/n-kalos/swap",
            json!({"with": "r-star", "version": v}),
        ),
    ] {
        assert_eq!(
            reads.refused("POST", path, body).await,
            (422, "invalid".into())
        );
    }
    assert_eq!(reads.version().await, v);
    assert!(reads.store.outbox_notices().await.unwrap().is_empty());
}

#[tokio::test]
async fn stale_planner_swap_changes_neither_run() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    reads
        .ok(
            "POST",
            "/api/admin/runs/n-kalos/move",
            json!({"day": 6, "time": "20:00", "version": v}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    let before = reads
        .read("/api/admin/week?week=next", "week.json#/$defs/Week")
        .await;
    assert_eq!(
        reads
            .refused(
                "POST",
                "/api/admin/runs/n-kalos/swap",
                json!({"with": "n-star", "version": v}),
            )
            .await,
        (409, "stale".into())
    );
    assert_eq!(
        reads
            .read("/api/admin/week?week=next", "week.json#/$defs/Week")
            .await,
        before
    );
    assert!(reads.store.outbox_notices().await.unwrap().len() == 1);
}

#[tokio::test]
async fn planner_swap_refuses_a_non_midnight_reset_week_escape() {
    let reset = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let reads = Reads::with_reset(reset).await;
    let mut version = reads.version().await;
    let moved = reads
        .ok(
            "POST",
            "/api/admin/runs/n-kalos/move",
            json!({"day": 1, "time": "10:00", "version": version}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    version = moved["version"].as_u64().unwrap();
    let moved = reads
        .ok(
            "POST",
            "/api/admin/runs/n-star/move",
            json!({"day": 0, "time": null, "version": version}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    version = moved["version"].as_u64().unwrap();
    let before = reads
        .read("/api/admin/week?week=next", "week.json#/$defs/Week")
        .await;
    assert_eq!(
        reads
            .refused(
                "POST",
                "/api/admin/runs/n-kalos/swap",
                json!({"with": "n-star", "version": version}),
            )
            .await,
        (422, "invalid".into())
    );
    assert_eq!(
        reads
            .read("/api/admin/week?week=next", "week.json#/$defs/Week")
            .await,
        before
    );
    assert_eq!(reads.version().await, version);
}

/// One edit of the merge property: `fields` are what it declares on
/// `r-kalos` (run edits) or, for a full-body weekly-timing PATCH, on `f-kalos`.
struct Edit {
    name: &'static str,
    fixed: bool,
    fields: &'static [&'static str],
    method: &'static str,
    path: &'static str,
    body: fn(u64) -> Value,
    result: &'static str,
}

const FIXED_FIELDS: [&str; 6] = ["day", "time", "bosses", "participants", "channel", "note"];

/// A weekly-timing PATCH as the PWA sends it: the whole form, as loaded at `v`.
fn timing(v: u64, time: &str, note: &str) -> Value {
    json!({
        "weekday": 1, "time": time, "bosses": "xkalos", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": note, "version": v,
    })
}

impl Reads {
    /// Cara has no bossing role, so no timing form could resend her: drop her
    /// from `f-kalos` (r-kalos keeps its own, amended roster).
    async fn drop_cara(&self) {
        let form = timing(self.version().await, "22:00", "bring pots");
        self.ok(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            form,
            "fixed.json#/$defs/FixedRow",
        )
        .await;
    }

    /// `r-kalos` and `f-kalos` as the admin screens show them.
    async fn rows(&self) -> (Value, Value) {
        let week = self.read("/api/admin/week", "week.json#/$defs/Week").await;
        let fixed = self
            .read("/api/admin/fixed", "fixed.json#/$defs/FixedRows")
            .await;
        let find = |rows: &Value, id: &str| {
            rows.as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap()
                .clone()
        };
        (find(&week["runs"], "r-kalos"), find(&fixed, "f-kalos"))
    }
}

/// Every pair of edits made from the same (head-first) week version, run
/// edits and weekly-timing edits alike: the second is refused exactly when
/// the first changed a field it declares (field-level merge, no false 409; a
/// same-field pair always conflicts, except an identical timing form, which
/// is a no-op), and a refused second edit never overwrites the first (no
/// lost update).
#[tokio::test]
async fn same_version_edits_merge_by_field_and_never_lose_an_update() {
    let run = |name, fields, method, path, body| Edit {
        name,
        fixed: false,
        fields,
        method,
        path,
        body,
        result: if path == "move" {
            "week.json#/$defs/MoveResult"
        } else {
            RUN_RESULT
        },
    };
    let fixed = |name, body| Edit {
        name,
        fixed: true,
        fields: &FIXED_FIELDS,
        method: "PATCH",
        path: "",
        body,
        result: "fixed.json#/$defs/FixedRow",
    };
    let edits = [
        run(
            "slot",
            &["slot"],
            "POST",
            "move",
            |v| json!({"day": 6, "time": "20:00", "version": v}),
        ),
        run(
            "status",
            &["status"],
            "PATCH",
            "status",
            |v| json!({"status": "confirmed", "version": v}),
        ),
        run(
            "participants",
            &["participants"],
            "PATCH",
            "participants",
            |v| json!({"remove": "1002", "version": v}),
        ),
        run(
            "rsvp:1001",
            &["rsvp:1001"],
            "POST",
            "rsvp",
            |v| json!({"member_id": "1001", "answer": "no", "version": v}),
        ),
        run(
            "rsvp:1002",
            &["rsvp:1002"],
            "POST",
            "rsvp",
            |v| json!({"member_id": "1002", "answer": "yes", "version": v}),
        ),
        run(
            "reset",
            &["slot", "participants", "bosses", "channel"],
            "POST",
            "reset",
            |v| json!({"version": v}),
        ),
        fixed("fixed time", |v| timing(v, "21:30", "bring pots")),
        fixed("fixed note", |v| timing(v, "22:00", "bring elixirs")),
    ];
    for first in &edits {
        for second in &edits {
            let reads = Reads::new().await;
            reads.drop_cara().await;
            // Dropping Cara synced r-kalos's roster; amend it again so a reset
            // has something to undo.
            let add = json!({"add": "1004", "version": reads.version().await});
            reads
                .ok(
                    "PATCH",
                    "/api/admin/runs/r-kalos/participants",
                    add,
                    RUN_RESULT,
                )
                .await;
            // What the PWA holds: the week's version, read before its data.
            let v = reads.read("/api/admin/week", "week.json#/$defs/Week").await["version"]
                .as_u64()
                .unwrap();
            let url = |edit: &Edit| {
                if edit.fixed {
                    "/api/admin/fixed/f-kalos".to_owned()
                } else {
                    format!("/api/admin/runs/r-kalos/{}", edit.path)
                }
            };
            let pair = format!("{} then {}", first.name, second.name);
            reads
                .ok(first.method, &url(first), (first.body)(v), first.result)
                .await;
            assert_eq!(
                reads.version().await,
                v + 1,
                "{pair}: the first edit applies"
            );
            let after_first = reads.rows().await;
            let reply = reads
                .call(second.method, &url(second), (second.body)(v), &[])
                .await;
            // What the first edit really changed (a roster or answer change may
            // re-derive the status, a timing edit moves the runs it updates).
            let changed = changed_fields(&reads.store.load_change(v + 1).await.unwrap().unwrap());
            let target = if second.fixed {
                BlameTarget::FixedRun("f-kalos".into())
            } else {
                BlameTarget::Run("r-kalos".into())
            };
            let touched = second
                .fields
                .iter()
                .any(|field| changed.contains(&(target.clone(), (*field).to_owned())));
            let identical = first.fixed && first.name == second.name;
            let conflicts = touched && !identical;
            assert!(
                first.name != second.name || conflicts || identical,
                "{pair}: an edit changes its own field"
            );
            if conflicts {
                assert_eq!(
                    (reply.status, reply.api_error()),
                    (409, "stale".into()),
                    "{pair}"
                );
                assert_eq!(
                    reads.rows().await,
                    after_first,
                    "{pair}: first edit survives"
                );
            } else if first.name == "slot" && second.name == "fixed time" {
                // The move amended r-kalos after the form was loaded, so moving
                // the timing now needs an update/keep for it: refused, nothing lost.
                assert_eq!(
                    (reply.status, reply.api_error()),
                    (422, "choices_required".into()),
                    "{pair}"
                );
                assert_eq!(reads.rows().await, after_first, "{pair}");
            } else {
                assert_eq!(reply.status, 200, "{pair}: {}", reply.text());
                assert_valid(second.result, &pair, &reply.json());
            }
        }
    }
}

/// The reviewer's case: B moves the timing, then A saves a note from the
/// form loaded before it. A's form still carries the old time, so it is
/// refused and B's time stands.
#[tokio::test]
async fn a_stale_timing_form_cannot_revert_another_edit() {
    let reads = Reads::new().await;
    reads.drop_cara().await;
    let v = reads.version().await;
    let b = timing(v, "21:30", "bring pots");
    assert_valid("fixed.json#/$defs/FixedRequest", "form", &b);
    reads
        .ok(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            b,
            "fixed.json#/$defs/FixedRow",
        )
        .await;
    let a = timing(v, "22:00", "bring elixirs");
    assert_eq!(
        reads.refused("PATCH", "/api/admin/fixed/f-kalos", a).await,
        (409, "stale".into())
    );
    let (_, timing) = reads.rows().await;
    assert_eq!(
        (timing["time"].clone(), timing["note"].clone()),
        ("21:30".into(), "bring pots".into())
    );
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
async fn rsvp_retracts_only_after_its_committed_write() {
    let reads = Reads::new().await;
    let version = reads.version().await;
    let body = json!({"member_id": "1002", "answer": "yes", "version": version});
    let first = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            body.clone(),
            &[("Idempotency-Key", "rsvp-retract")],
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(
        reads.decline_retractions.lock().unwrap().as_slice(),
        [("r-kalos".into(), "1002".into())]
    );

    let replay = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            body,
            &[("Idempotency-Key", "rsvp-retract")],
        )
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(reads.decline_retractions.lock().unwrap().len(), 1);

    let refused = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1006", "answer": "yes", "version": reads.version().await}),
            &[("Idempotency-Key", "rsvp-refused")],
        )
        .await;
    assert_eq!(refused.status, 422, "{}", refused.text());
    assert_eq!(reads.decline_retractions.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn decline_rsvp_idempotency_binds_expectations_and_replays_once() {
    let reads = Reads::new().await;
    let version = reads.version().await;
    let body = |seen| {
        json!({
            "member_id": "1004",
            "answer": "no",
            "version": version,
            "expect": [{"field": "rsvp:1004", "seen": seen}]
        })
    };
    let key = [("Idempotency-Key", "decline-expectations")];

    let first = reads
        .call("POST", "/api/admin/runs/r-kalos/rsvp", body(None), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(reads.version().await, version + 1);
    let candidate = reads
        .store
        .decline_notice("r-kalos", "1004")
        .await
        .expect("decline notice")
        .expect("first decline candidate");

    let different_expectation = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            body(Some(version)),
            &key,
        )
        .await;
    assert_eq!(
        (
            different_expectation.status,
            different_expectation.api_error()
        ),
        (422, "idempotency_mismatch".into())
    );

    let replay = reads
        .call("POST", "/api/admin/runs/r-kalos/rsvp", body(None), &key)
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(reads.version().await, version + 1, "one history record");
    assert_eq!(
        reads
            .store
            .decline_notice("r-kalos", "1004")
            .await
            .expect("decline notice"),
        Some(candidate),
        "replay does not replace the candidate"
    );
    assert_eq!(
        reads
            .store
            .pending_decline_notices(10)
            .await
            .expect("pending declines")
            .len(),
        1
    );
}

#[tokio::test]
async fn a_no_op_yes_does_not_retract_a_pending_decline() {
    let reads = Reads::new().await;
    let first = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1001", "answer": "yes", "version": reads.version().await}),
            &[],
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert!(reads.decline_retractions.lock().unwrap().is_empty());

    let first_version = reads.version().await;
    let at = reads
        .store
        .load_change(first_version)
        .await
        .expect("first RSVP history")
        .expect("first RSVP record")
        .at;
    reads
        .store
        .commit_with_decline_notices(
            first_version,
            ChangeSet {
                changes: vec![Change::PutRsvp(Rsvp {
                    run_id: "r-kalos".into(),
                    user_id: "1004".into(),
                    state: RsvpState::Maybe,
                    source: RsvpSource::Reaction,
                    at,
                })],
            },
            ChangeMeta {
                origin: Origin::for_tests(),
                at,
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            },
            vec![DeclineNotice::candidate(
                "r-kalos",
                "1001",
                Some("kalos-four".into()),
                None,
                "Alice",
                at,
            )],
            Vec::new(),
        )
        .await
        .expect("seed pending decline");
    let version = reads.version().await;
    let candidate = reads
        .store
        .decline_notice("r-kalos", "1001")
        .await
        .expect("decline notice")
        .expect("pending decline candidate");
    assert!(!candidate.retract_pending);

    let repeated = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/rsvp",
            json!({"member_id": "1001", "answer": "yes", "version": version}),
            &[],
        )
        .await;
    assert_eq!(repeated.status, 200, "{}", repeated.text());
    assert_eq!(
        reads.version().await,
        version,
        "the repeated answer is a no-op"
    );
    assert!(reads.decline_retractions.lock().unwrap().is_empty());
    assert_eq!(
        reads
            .store
            .decline_notice("r-kalos", "1001")
            .await
            .expect("decline notice"),
        Some(candidate)
    );
}

#[tokio::test]
async fn weekly_timings_create_edit_with_decisions_and_retire() {
    let reads = Reads::new().await;
    let create = json!({
        "weekday": 3, "time": "21:00", "bosses": "hstar", "participants": ["1001", "1004"],
        "channel_id": "kalos-four", "note": " "
    });
    assert_valid("fixed.json#/$defs/FixedRequest", "create", &create);
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
    let mut edit = json!({
        "weekday": 1, "time": "21:30", "bosses": "xkalos", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": "bring pots"
    });
    assert_eq!(
        reads
            .refused("PATCH", "/api/admin/fixed/f-kalos", edit.clone())
            .await,
        (422, "version_required".into())
    );
    edit["version"] = reads.version().await.into();
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
    wrong["version"] = reads.version().await.into();
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
        ("all".into(), Value::Null)
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

#[tokio::test]
async fn alias_removal_is_idempotent_and_keeps_order() {
    let reads = Reads::new().await;
    for alias in ["bobby", "rob", "bob-2"] {
        reads
            .ok(
                "POST",
                "/api/admin/members/1002/aliases",
                json!({ "alias": alias }),
                "members.json#/$defs/MemberRow",
            )
            .await;
    }
    // The path alias is percent-decoded, trimmed and lowercased.
    let row = reads
        .ok(
            "DELETE",
            "/api/admin/members/1002/aliases/%20Rob%20",
            json!({}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(row["aliases"], json!(["bobby", "bob-2"]));
    let again = reads
        .ok(
            "DELETE",
            "/api/admin/members/1002/aliases/rob",
            json!({}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(again, row, "an alias not held leaves the row unchanged");
    let stored = reads.store.load_member("1002").await.unwrap().unwrap();
    assert_eq!(stored.aliases, ["bobby", "bob-2"]);
    // Another member's alias is not this member's to drop.
    reads
        .ok(
            "DELETE",
            "/api/admin/members/1002/aliases/ali",
            json!({}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    let alice = reads.store.load_member("1001").await.unwrap().unwrap();
    assert_eq!(alice.aliases, ["ali"]);
    // Released: another member may take it.
    let row = reads
        .ok(
            "POST",
            "/api/admin/members/1001/aliases",
            json!({"alias": "rob"}),
            "members.json#/$defs/MemberRow",
        )
        .await;
    assert_eq!(row["aliases"], json!(["ali", "rob"]));

    assert_eq!(
        reads
            .refused("DELETE", "/api/admin/members/9999/aliases/rob", json!({}))
            .await,
        (404, "not_found".into())
    );
    let no_csrf = send(
        reads.admin,
        "DELETE",
        ADMIN_HOST,
        "/api/admin/members/1002/aliases/bobby",
        &[("Cookie", reads.cookie.as_str()), ORIGIN],
        None,
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    let stored = reads.store.load_member("1002").await.unwrap().unwrap();
    assert_eq!(stored.aliases, ["bobby", "bob-2"], "nothing written");
}

/// With a 05:00 reset, day 0 (Thursday) before 05:00 is the previous boss
/// week: a move there is refused, not silently re-weeked.
#[tokio::test]
async fn a_move_never_leaves_the_boss_week() {
    let reset = chrono::NaiveTime::from_hms_opt(5, 0, 0).unwrap();
    let reads = Reads::with_reset(reset).await;
    let v = reads.version().await;
    assert_eq!(
        reads
            .refused(
                "POST",
                "/api/admin/runs/r-kalos/move",
                json!({"day": 0, "time": "03:00", "version": v}),
            )
            .await,
        (422, "invalid".into())
    );
    assert_eq!(reads.version().await, v);
    let moved = reads
        .ok(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 0, "time": "06:00", "version": v}),
            "week.json#/$defs/MoveResult",
        )
        .await;
    assert_eq!(
        (moved["run"]["day"].clone(), moved["run"]["time"].clone()),
        (0.into(), "06:00".into())
    );
}

/// Replays are matched before validation: a participant who lost the
/// bossing role since the first attempt must not turn the retry into a 422,
/// while a new request with the same data is refused as usual.
#[tokio::test]
async fn timing_replays_survive_a_lost_role() {
    let reads = Reads::new().await;
    let create = json!({
        "weekday": 3, "time": "21:00", "bosses": "hstar", "participants": ["1001", "1004"],
        "channel_id": "kalos-four", "note": null
    });
    let created = reads
        .call(
            "POST",
            "/api/admin/fixed",
            create.clone(),
            &[("Idempotency-Key", "c-1")],
        )
        .await;
    assert_eq!(created.status, 201, "{}", created.text());
    let id = created.json()["id"].as_str().unwrap().to_owned();
    reads.drop_cara().await;
    let v = reads.version().await;
    let edit = timing(v, "22:00", "bring elixirs");
    let edited = reads
        .call(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            edit.clone(),
            &[("Idempotency-Key", "e-1")],
        )
        .await;
    assert_eq!(edited.status, 200, "{}", edited.text());

    // Dan (on the new timing) and Bob (on f-kalos) lose the bossing role.
    for user in ["1004", "1002"] {
        let mut profile = reads
            .store
            .list_members()
            .await
            .unwrap()
            .into_iter()
            .find(|profile| profile.member.user_id == user)
            .unwrap();
        profile.member.has_role = false;
        reads.store.put_member(profile).await.unwrap();
    }
    let replay = reads
        .call(
            "POST",
            "/api/admin/fixed",
            create.clone(),
            &[("Idempotency-Key", "c-1")],
        )
        .await;
    assert_eq!(replay.status, 201, "{}", replay.text());
    assert_eq!(replay.json()["id"], id.as_str());
    let replay = reads
        .call(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            edit.clone(),
            &[("Idempotency-Key", "e-1")],
        )
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["note"], "bring elixirs");
    assert_eq!(reads.version().await, v + 1, "nothing applied twice");

    // A different body under a used key is still a mismatch; a fresh key is validated.
    let mut other = create.clone();
    other["time"] = "20:00".into();
    let reused = reads
        .call(
            "POST",
            "/api/admin/fixed",
            other,
            &[("Idempotency-Key", "c-1")],
        )
        .await;
    assert_eq!(
        (reused.status, reused.api_error()),
        (422, "idempotency_mismatch".into())
    );
    let fresh = reads
        .call(
            "POST",
            "/api/admin/fixed",
            create,
            &[("Idempotency-Key", "c-2")],
        )
        .await;
    assert_eq!((fresh.status, fresh.api_error()), (422, "invalid".into()));
}

/// A retire retry is matched on the recorded change, not on the scheduler's
/// digest (which names the materialised weeks and so moves at the reset).
#[tokio::test]
async fn retire_replays_by_record() {
    let reads = Reads::new().await;
    let key = [("Idempotency-Key", "retire-1")];
    let first = reads
        .call("DELETE", "/api/admin/fixed/f-kalos", json!({}), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(first.json()["cancelled"], 2, "this and next week's runs");
    let v = reads.version().await;
    let again = reads
        .call("DELETE", "/api/admin/fixed/f-kalos", json!({}), &key)
        .await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_valid("fixed.json#/$defs/FixedRetired", "replay", &again.json());
    assert_eq!(again.json()["cancelled"], 0);
    assert_eq!(reads.version().await, v);
    let elsewhere = reads
        .call("DELETE", "/api/admin/fixed/other", json!({}), &key)
        .await;
    assert_eq!(
        (elsewhere.status, elsewhere.api_error()),
        (422, "idempotency_mismatch".into())
    );
}
