//! A4 admin mutations against the seeded store of `reads.rs` (pinned clock
//! Tue 29 Sep 12:00 KL; `r-kalos` is Tue 22:00 this week, amended off its
//! weekly timing `f-kalos`). Every response is validated against A0.

use chrono::{TimeZone, Utc};
use kanade::domain::{
    history::{BlameTarget, ChangeHistory, ChangeMeta, Origin, Surface, changed_fields},
    members::MemberStore,
    notify::{DeclineNotice, DeclineNoticeStore, NoticeOutbox},
    schedule::{Change, ChangeSet, Rsvp, RsvpSource, RsvpState},
    scheduler::{ScheduleStore, Scope},
};
use serde_json::{Value, json};

use crate::{
    reads::{Reads, commit_as},
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, fields, find, send},
};

const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const RUN_RESULT: &str = "week.json#/$defs/RunResult";
const MOVE_RESULT: &str = "week.json#/$defs/MoveResult";
const WEEK: &str = "week.json#/$defs/Week";
const FIXED_ROW: &str = "fixed.json#/$defs/FixedRow";
const MEMBER_ROW: &str = "members.json#/$defs/MemberRow";
const ERROR: &str = "error.json#/$defs/ApiError";
const FIXED: &str = "/api/admin/fixed";
const F_KALOS: &str = "/api/admin/fixed/f-kalos";
const MOVE_KALOS: &str = "/api/admin/runs/r-kalos/move";
const RSVP_KALOS: &str = "/api/admin/runs/r-kalos/rsvp";
const MOVE_NKALOS: &str = "/api/admin/runs/n-kalos/move";
const SWAP: &str = "/api/admin/runs/n-kalos/swap";
const NEXT_WEEK: &str = "/api/admin/week?week=next";
const BOB_ALIASES: &str = "/api/admin/members/1002/aliases";

mod fixed_patch_replay;

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

    /// [`Reads::call`] under the `Idempotency-Key` `key`.
    pub(crate) async fn keyed(&self, method: &str, path: &str, key: &str, body: Value) -> Reply {
        self.call(method, path, body, &[("Idempotency-Key", key)])
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

    /// Signed in but without the CSRF token.
    pub(crate) async fn without_csrf(&self, method: &str, path: &str, body: &Value) -> Reply {
        let headers = [("Cookie", self.cookie.as_str()), ORIGIN];
        let body = body.to_string();
        send(self.admin, method, ADMIN_HOST, path, &headers, Some(&body)).await
    }

    /// Signed out (no session cookie).
    pub(crate) async fn anonymous(&self, method: &str, path: &str, body: &Value) -> Reply {
        let body = body.to_string();
        send(self.admin, method, ADMIN_HOST, path, &[ORIGIN], Some(&body)).await
    }

    /// An unmounted route: the generic 404 signed in (sending `body`) and
    /// before the session check.
    pub(crate) async fn assert_unmounted(&self, method: &str, path: &str, body: Value) {
        let signed_in = self.call(method, path, body, &[]).await;
        assert_eq!(
            signed_in.error(),
            (404, "not_found".into()),
            "{method} {path}"
        );
        let anonymous = send(self.admin, method, ADMIN_HOST, path, &[ORIGIN], Some("{}")).await;
        assert_eq!(anonymous.status, 404, "{method} {path} signed out");
        assert_valid(ERROR, path, &anonymous.json());
    }
}

#[tokio::test]
async fn moves_are_attributed_to_the_session_and_csrf_is_required() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let body = json!({"day": 6, "time": "21:00", "version": v});
    let reply = reads
        .ok("POST", MOVE_KALOS, body.clone(), MOVE_RESULT)
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
    let no_csrf = reads.without_csrf("POST", MOVE_KALOS, &body).await;
    assert_eq!(no_csrf.error(), (403, "csrf".into()));
    let anonymous = reads.anonymous("POST", MOVE_KALOS, &body).await;
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
    let before = reads.read(NEXT_WEEK, WEEK).await;
    let run = |id: &str| find(&before["runs"], "id", id).clone();
    let kalos = run("n-kalos");
    let own_time = run("n-star");
    let body = json!({"with": "n-star", "version": v});
    let key = [("Idempotency-Key", "planner-swap-1")];
    let swapped = reads.call("POST", SWAP, body.clone(), &key).await;
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

    let replay = reads.call("POST", SWAP, body, &key).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json(), swapped, "same response on replay");
    assert_eq!(reads.version().await, v + 1, "one record only");
    assert_eq!(reads.store.outbox_notices().await.unwrap().len(), 2);
    let body = json!({"with": "n-star", "version": v + 1});
    let changed_version = reads.call("POST", SWAP, body, &key).await;
    assert_eq!(
        changed_version.error(),
        (422, "idempotency_mismatch".into())
    );
    assert_eq!(
        reads.version().await,
        v + 1,
        "changed-version reuse writes nothing"
    );

    let revert = json!({"seqs": [v + 1]});
    let plan = "history.json#/$defs/RevertPlan";
    reads
        .ok("POST", "/api/admin/history/revert", revert, plan)
        .await;
    let restored = reads.read(NEXT_WEEK, WEEK).await;
    for original in [kalos, own_time] {
        let current = find(&restored["runs"], "id", original["id"].as_str().unwrap());
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
    for with in ["n-kalos", "r-kalos", "r-star"] {
        let body = json!({"with": with, "version": v});
        let refusal = reads.refused("POST", SWAP, body).await;
        assert_eq!(refusal, (422, "invalid".into()), "{with}");
    }
    assert_eq!(reads.version().await, v);
    assert!(reads.store.outbox_notices().await.unwrap().is_empty());
}

#[tokio::test]
async fn stale_planner_swap_changes_neither_run() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let body = json!({"day": 6, "time": "20:00", "version": v});
    reads.ok("POST", MOVE_NKALOS, body, MOVE_RESULT).await;
    let before = reads.read(NEXT_WEEK, WEEK).await;
    let body = json!({"with": "n-star", "version": v});
    let refusal = reads.refused("POST", SWAP, body).await;
    assert_eq!(refusal, (409, "stale".into()));
    assert_eq!(reads.read(NEXT_WEEK, WEEK).await, before);
    assert!(reads.store.outbox_notices().await.unwrap().len() == 1);
}

#[tokio::test]
async fn planner_swap_refuses_a_non_midnight_reset_week_escape() {
    let reset = chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap();
    let reads = Reads::with_reset(reset).await;
    let version = reads.version().await;
    let body = json!({"day": 1, "time": "10:00", "version": version});
    let moved = reads.ok("POST", MOVE_NKALOS, body, MOVE_RESULT).await;
    let body = json!({"day": 0, "time": null, "version": moved["version"]});
    let path = "/api/admin/runs/n-star/move";
    let moved = reads.ok("POST", path, body, MOVE_RESULT).await;
    let version = moved["version"].as_u64().unwrap();
    let before = reads.read(NEXT_WEEK, WEEK).await;
    let body = json!({"with": "n-star", "version": version});
    let refusal = reads.refused("POST", SWAP, body).await;
    assert_eq!(refusal, (422, "invalid".into()));
    assert_eq!(reads.read(NEXT_WEEK, WEEK).await, before);
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
        self.ok("PATCH", F_KALOS, form, FIXED_ROW).await;
    }

    /// `r-kalos` and `f-kalos` as the admin screens show them.
    async fn rows(&self) -> (Value, Value) {
        let week = self.read("/api/admin/week", WEEK).await;
        let fixed = self.read(FIXED, "fixed.json#/$defs/FixedRows").await;
        (
            find(&week["runs"], "id", "r-kalos").clone(),
            find(&fixed, "id", "f-kalos").clone(),
        )
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
            MOVE_RESULT
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
        result: FIXED_ROW,
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
            let path = "/api/admin/runs/r-kalos/participants";
            reads.ok("PATCH", path, add, RUN_RESULT).await;
            // What the PWA holds: the week's version, read before its data.
            let v = reads.read("/api/admin/week", WEEK).await["version"]
                .as_u64()
                .unwrap();
            let url = |edit: &Edit| {
                if edit.fixed {
                    F_KALOS.to_owned()
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
                assert_eq!(reply.error(), (409, "stale".into()), "{pair}");
                assert_eq!(
                    reads.rows().await,
                    after_first,
                    "{pair}: first edit survives"
                );
            } else if first.name == "slot" && second.name == "fixed time" {
                // The move amended r-kalos after the form was loaded, so moving
                // the timing now needs an update/keep for it: refused, nothing lost.
                assert_eq!(reply.error(), (422, "choices_required".into()), "{pair}");
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
    reads.ok("PATCH", F_KALOS, b, FIXED_ROW).await;
    let a = timing(v, "22:00", "bring elixirs");
    assert_eq!(
        reads.refused("PATCH", F_KALOS, a).await,
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
    let first = reads.call("POST", MOVE_KALOS, body.clone(), &key).await;
    assert_eq!(first.status, 200);
    // A retry after the slot moved (by itself) replays instead of 409.
    let again = reads.call("POST", MOVE_KALOS, body, &key).await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_eq!(again.json()["run"], first.json()["run"]);
    assert_eq!(reads.version().await, v + 1, "applied once");
    // A fresh key from the same stale version is a real conflict, not a replay.
    let body = json!({"day": 4, "time": "21:00", "version": v});
    let stale = reads.keyed("POST", MOVE_KALOS, "move-2", body).await;
    assert_eq!(stale.error(), (409, "stale".into()));

    let other = json!({"day": 4, "time": "21:00", "version": v});
    let reused = reads.call("POST", MOVE_KALOS, other, &key).await;
    assert_eq!(reused.error(), (422, "idempotency_mismatch".into()));
    let body = json!({"day": 1, "time": "21:00", "version": v});
    let bad = reads.keyed("POST", MOVE_KALOS, "no spaces", body).await;
    assert_eq!(bad.error(), (400, "invalid_idempotency_key".into()));
}

#[tokio::test]
async fn explicit_expectations_and_admin_overrides() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    let body = json!({"day": 6, "time": "21:00", "version": v});
    reads.ok("POST", MOVE_KALOS, body, MOVE_RESULT).await;
    let head = reads.store.history_head().await.unwrap();
    // Declaring the stale `seen` is a conflict; overriding the change it names applies.
    let stale =
        json!({"day": 4, "time": "21:00", "version": v, "expect": [{"field": "slot", "seen": v}]});
    assert_eq!(
        reads.refused("POST", MOVE_KALOS, stale).await,
        (409, "stale".into())
    );
    let unknown = json!({"day": 4, "time": "21:00", "version": v, "expect": [{"field": "colour", "seen": null}]});
    assert_eq!(
        reads.refused("POST", MOVE_KALOS, unknown).await,
        (422, "unknown_field".into())
    );
    let forced = json!({
        "day": 4, "time": "21:00", "version": v,
        "expect": [{"field": "slot", "seen": head.seq}],
        "override": [{"seq": head.seq, "hash": head.hash}],
    });
    let reply = reads.ok("POST", MOVE_KALOS, forced, MOVE_RESULT).await;
    assert_eq!(reply["run"]["day"], 4);
}

#[tokio::test]
async fn run_edits_refuse_with_the_table_codes() {
    let reads = Reads::new().await;
    let v = reads.version().await;
    // Each body is sent at the current `version`.
    let invalid = [
        ("PATCH", "r-kalos/status", json!({"status": "sideways"})),
        ("PATCH", "r-kalos/status", json!({"status": "at_risk"})),
        ("POST", "r-kalos/move", json!({"day": 7, "time": "21:00"})),
        ("POST", "r-kalos/move", json!({"day": 1, "time": "9pm"})),
        ("POST", "r-star/move", json!({"day": 1, "time": "21:00"})),
        ("PATCH", "r-kalos/participants", json!({"add": "1006"})),
        ("PATCH", "r-kalos/participants", json!({})),
    ]
    .map(|(method, run, body)| (method, run, body, 422, "invalid"));
    let others = [
        (
            "PATCH",
            "nope/status",
            json!({"status": "done"}),
            404,
            "not_found",
        ),
        (
            "POST",
            "r-kalos/rsvp",
            json!({"member_id": "1006", "answer": "yes"}),
            422,
            "not_on_run",
        ),
        (
            "PATCH",
            "r-kalos/status",
            json!({"status": "done", "colour": 1}),
            400,
            "invalid_body",
        ),
    ];
    for (method, run, mut body, status, code) in invalid.into_iter().chain(others) {
        body["version"] = v.into();
        let path = format!("/api/admin/runs/{run}");
        assert_eq!(
            reads.refused(method, &path, body.clone()).await,
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
    let body = json!({"member_id": "1004", "answer": "yes", "version": v});
    let run = reads.ok("POST", RSVP_KALOS, body, RUN_RESULT).await;
    assert_eq!(answer(&run, "1004"), Some("yes".into()));
    let body = json!({"member_id": "1001", "answer": "clear", "version": v});
    let run = reads.ok("POST", RSVP_KALOS, body, RUN_RESULT).await;
    assert_eq!(answer(&run, "1001"), Some("waiting".into()));

    let body = json!({"remove": "1004", "version": v});
    let path = "/api/admin/runs/r-kalos/participants";
    let run = reads.ok("PATCH", path, body, RUN_RESULT).await;
    assert_eq!(answer(&run, "1004"), None);

    let body = json!({"version": reads.version().await});
    let path = "/api/admin/runs/r-kalos/reset";
    let run = reads.ok("POST", path, body, RUN_RESULT).await;
    assert_eq!(run["run"]["amended"], false);
    assert_eq!(run["run"]["roster_change"], Value::Null);

    let message = "common.json#/$defs/Message";
    let ping = reads.ok("POST", "/api/admin/runs/r-kalos/ping", json!({}), message);
    let ping = ping.await;
    // The run sheet shows this wording verbatim.
    let message = ping["message"].as_str().unwrap();
    assert!(
        message.starts_with("Preview (not posted): the morning card for ")
            && message.ends_with('.'),
        "{message}"
    );
    let refusal = reads.refused("POST", "/api/admin/runs/nope/ping", json!({}));
    assert_eq!(refusal.await, (404, "not_found".into()));
}

#[tokio::test]
async fn rsvp_retracts_only_after_its_committed_write() {
    let reads = Reads::new().await;
    let version = reads.version().await;
    let body = json!({"member_id": "1002", "answer": "yes", "version": version});
    let first = reads
        .keyed("POST", RSVP_KALOS, "rsvp-retract", body.clone())
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(
        reads.decline_retractions.lock().unwrap().as_slice(),
        [("r-kalos".into(), "1002".into())]
    );

    let replay = reads.keyed("POST", RSVP_KALOS, "rsvp-retract", body).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(reads.decline_retractions.lock().unwrap().len(), 1);

    let body = json!({"member_id": "1006", "answer": "yes", "version": reads.version().await});
    let refused = reads.keyed("POST", RSVP_KALOS, "rsvp-refused", body).await;
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

    let first = reads.call("POST", RSVP_KALOS, body(None), &key).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(reads.version().await, version + 1);
    let candidate = reads
        .store
        .decline_notice("r-kalos", "1004")
        .await
        .expect("decline notice")
        .expect("first decline candidate");

    let different_expectation = reads
        .call("POST", RSVP_KALOS, body(Some(version)), &key)
        .await;
    assert_eq!(
        different_expectation.error(),
        (422, "idempotency_mismatch".into())
    );

    let replay = reads.call("POST", RSVP_KALOS, body(None), &key).await;
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
    let body = json!({"member_id": "1001", "answer": "yes", "version": reads.version().await});
    let first = reads.call("POST", RSVP_KALOS, body, &[]).await;
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

    let body = json!({"member_id": "1001", "answer": "yes", "version": version});
    let repeated = reads.call("POST", RSVP_KALOS, body, &[]).await;
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
    let head = || async {
        kanade::domain::history::ChangeHistory::history_head(&*reads.store)
            .await
            .unwrap()
            .seq
    };
    let before = head().await;
    let created = reads.call("POST", FIXED, create.clone(), &key).await;
    assert_eq!(created.status, 201, "{}", created.text());
    assert_eq!(
        head().await,
        before + 1,
        "the timing and its runs are one change record"
    );
    let row = created.json();
    assert_valid(FIXED_ROW, "create", &row);
    assert_eq!(row["weekday_name"], "Thursday");
    assert_eq!(row["bosses"][0]["token"], "HMaleficStar");
    assert_eq!(row["note"], Value::Null);
    assert_eq!(
        row["runs"][0]["week"], "next",
        "this week's Thursday has passed"
    );
    let replay = reads.call("POST", FIXED, create.clone(), &key).await;
    assert_eq!(replay.status, 201);
    assert_eq!(replay.json()["id"], row["id"]);
    assert_eq!(head().await, before + 1, "a replay records nothing");
    let rows = reads.read(FIXED, "fixed.json#/$defs/FixedRows").await;
    assert_eq!(rows.as_array().unwrap().len(), 2, "created once");

    let mut unwatched = create.clone();
    unwatched["channel_id"] = "star".into();
    assert_eq!(reads.refused("POST", FIXED, unwatched).await.0, 422);
    let mut unknown_boss = create.clone();
    unknown_boss["bosses"] = "hwhatever".into();
    assert_eq!(
        reads.refused("POST", FIXED, unknown_boss).await,
        (422, "invalid".into())
    );

    // f-kalos: Tue 22:00. Move this week's run off it; moving the timing then needs a decision.
    let v = reads.version().await;
    let body = json!({"day": 5, "time": "23:00", "version": v});
    reads.ok("POST", MOVE_KALOS, body, MOVE_RESULT).await;
    let mut edit = json!({
        "weekday": 1, "time": "21:30", "bosses": "xkalos", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": "bring pots"
    });
    assert_eq!(
        reads.refused("PATCH", F_KALOS, edit.clone()).await,
        (422, "version_required".into())
    );
    edit["version"] = reads.version().await.into();
    assert_eq!(
        reads.refused("PATCH", F_KALOS, edit.clone()).await,
        (422, "choices_required".into())
    );
    let mut kept = edit.clone();
    kept["decisions"] = json!({"r-kalos": "keep"});
    let row = reads.ok("PATCH", F_KALOS, kept, FIXED_ROW).await;
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
        reads.refused("PATCH", F_KALOS, wrong).await.1,
        "choices_not_applicable"
    );

    let retired = "fixed.json#/$defs/FixedRetired";
    let retired = reads.ok("DELETE", F_KALOS, json!({}), retired).await;
    assert_eq!(
        retired["cancelled"], 3,
        "this, next and the materialised week after"
    );
    assert_eq!(
        reads.refused("DELETE", F_KALOS, json!({})).await,
        (404, "not_found".into())
    );
    assert_eq!(
        reads.refused("PATCH", F_KALOS, edit).await,
        (404, "not_found".into())
    );

    let path = "/api/admin/validate/bosses";
    let result = "fixed.json#/$defs/ValidateResult";
    let valid = reads.ok("POST", path, json!({"text": "xkalos, hstar"}), result);
    assert_eq!(valid.await["bosses"].as_array().unwrap().len(), 2);
    let refusal = reads.refused("POST", path, json!({"text": "kalos"})).await;
    assert_eq!(refusal.1, "invalid");
}

/// An amended run that has already ended is frozen: a day/time edit leaves
/// it alone, so the timing's read must not offer it as a Keep/Update choice
/// (it did, and the save the editor then sent was `choices_not_applicable`).
#[tokio::test]
async fn an_ended_amended_run_is_not_offered_as_a_timing_choice() {
    let reads = Reads::build(|b| b.run_ends = true).await;
    let snapshot = reads.store.load(&Scope::All).await.unwrap();
    let mut early = snapshot
        .runs
        .iter()
        .find(|run| run.id == "r-kalos")
        .unwrap()
        .clone();
    // Tue 09:00 KL, off its 22:00 timing and over by the pinned 12:00.
    early.datetime = Utc.with_ymd_and_hms(2026, 9, 29, 1, 0, 0).unwrap();
    let at = Utc.with_ymd_and_hms(2026, 9, 29, 3, 0, 0).unwrap();
    commit_as(&reads.store, "test", at, vec![Change::PutRun(early)]).await;

    let amended = |row: &Value| -> Vec<(String, String, bool)> {
        row["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|run| {
                (
                    run["run_id"].as_str().unwrap().to_owned(),
                    run["time"].as_str().unwrap().to_owned(),
                    run["amended"].as_bool().unwrap(),
                )
            })
            .collect()
    };
    let (_, row) = reads.rows().await;
    assert_eq!(
        amended(&row),
        [
            ("r-kalos".into(), "09:00".into(), false),
            ("n-kalos".into(), "22:00".into(), false)
        ],
        "the ended run is not offered"
    );

    // What the editor sends with nothing to ask: no decisions. Cara has lost
    // the bossing role, so the party drops her (a change the ended run skips).
    let mut edit = json!({
        "weekday": 1, "time": "21:30", "bosses": "xkalos", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": "bring pots"
    });
    edit["version"] = reads.version().await.into();
    let row = reads.ok("PATCH", F_KALOS, edit, FIXED_ROW).await;
    assert_eq!(
        amended(&row),
        [
            ("r-kalos".into(), "09:00".into(), false),
            ("n-kalos".into(), "21:30".into(), false)
        ],
        "the ended run keeps its slot; next week's follows the timing"
    );
}

/// The editor asks update or keep only about the runs the save lists. It
/// asked about every amended run, so a save keeping the day and time, or one
/// with a run changed only in its roster, was refused `choices_not_applicable`.
#[tokio::test]
async fn timing_choices_are_the_runs_the_save_takes_decisions_for() {
    let reads = Reads::new().await;
    let path = "/api/admin/fixed/f-kalos/choices";
    let listed = async |body: Value| -> Vec<String> {
        assert_valid("fixed.json#/$defs/FixedChoicesRequest", "choices", &body);
        let choices = reads.ok("POST", path, body, "fixed.json#/$defs/FixedChoices");
        let runs = choices.await["runs"].as_array().unwrap().clone();
        runs.iter()
            .map(|run| run["run_id"].as_str().unwrap().to_owned())
            .collect()
    };
    // f-kalos: Tue 22:00. r-kalos moves off it; n-kalos changes only its roster.
    let v = reads.version().await;
    let body = json!({"day": 5, "time": "23:00", "version": v});
    reads.ok("POST", MOVE_KALOS, body, MOVE_RESULT).await;
    let add = json!({"add": "1004", "version": reads.version().await});
    let roster = "/api/admin/runs/n-kalos/participants";
    reads.ok("PATCH", roster, add, RUN_RESULT).await;
    let (_, row) = reads.rows().await;
    let amended: Vec<_> = row["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| (run["run_id"].clone(), run["amended"].clone()))
        .collect();
    assert_eq!(
        amended,
        [
            ("r-kalos".into(), true.into()),
            ("n-kalos".into(), true.into())
        ]
    );

    let unchanged = listed(json!({"weekday": 1, "time": "22:00"})).await;
    assert!(
        unchanged.is_empty(),
        "day and time unchanged: {unchanged:?}"
    );
    let moved = listed(json!({"weekday": 1, "time": "21:30"})).await;
    assert_eq!(moved, ["r-kalos"], "not the roster-only run");
    assert_eq!(
        reads
            .refused("POST", path, json!({"weekday": 1, "time": "9:30"}))
            .await,
        (422, "invalid".into())
    );
    let gone = "/api/admin/fixed/f-gone/choices";
    assert_eq!(
        reads
            .refused("POST", gone, json!({"weekday": 1, "time": "21:30"}))
            .await,
        (404, "not_found".into())
    );

    // What the editor then sends: no decisions for a note-only save, and one
    // per listed run for the time change.
    let note = timing(reads.version().await, "22:00", "bring pots");
    reads.ok("PATCH", F_KALOS, note, FIXED_ROW).await;
    let mut moved = timing(reads.version().await, "21:30", "bring pots");
    moved["decisions"] = json!({"r-kalos": "keep"});
    let row = reads.ok("PATCH", F_KALOS, moved, FIXED_ROW).await;
    let times: Vec<_> = row["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| (run["run_id"].clone(), run["time"].clone()))
        .collect();
    assert_eq!(
        times,
        [
            ("r-kalos".into(), "23:00".into()),
            ("n-kalos".into(), "21:30".into())
        ]
    );
}

#[tokio::test]
async fn member_edits_and_aliases() {
    let reads = Reads::new().await;
    let alice = "/api/admin/members/1001";
    let body = json!({"ping_level": "all", "persona": "default"});
    let row = reads.ok("PATCH", alice, body, MEMBER_ROW).await;
    assert_eq!(
        (row["ping_level"].clone(), row["persona"].clone()),
        ("all".into(), Value::Null)
    );
    let row = reads
        .ok("PATCH", alice, json!({"persona": ""}), MEMBER_ROW)
        .await;
    assert_eq!(row["persona"], Value::Null);
    assert_eq!(row["aliases"], json!(["ali"]), "untouched");

    let body = json!({"alias": " Bobby "});
    let row = reads.ok("POST", BOB_ALIASES, body, MEMBER_ROW).await;
    assert_eq!(row["aliases"], json!(["bobby"]));
    let bob = "/api/admin/members/1002";
    for (method, path, body, status, code) in [
        (
            "POST",
            BOB_ALIASES,
            json!({"alias": "ali"}),
            422,
            "alias_taken",
        ),
        (
            "POST",
            BOB_ALIASES,
            json!({"alias": "two words"}),
            422,
            "invalid",
        ),
        ("PATCH", bob, json!({"ping_level": "loud"}), 422, "invalid"),
        ("PATCH", bob, json!({"persona": "pirate"}), 422, "invalid"),
        (
            "PATCH",
            "/api/admin/members/9999",
            json!({"ping_level": "all"}),
            404,
            "not_found",
        ),
    ] {
        assert_eq!(
            reads.refused(method, path, body).await,
            (status, code.into()),
            "{path}"
        );
    }
}

impl Reads {
    /// DELETE `alias` from Bob (as sent, before decoding): his row.
    async fn drop_alias(&self, alias: &str) -> Value {
        let path = format!("{BOB_ALIASES}/{alias}");
        self.ok("DELETE", &path, json!({}), MEMBER_ROW).await
    }
}

#[tokio::test]
async fn alias_removal_is_idempotent_and_keeps_order() {
    let reads = Reads::new().await;
    for alias in ["bobby", "rob", "bob-2"] {
        let body = json!({ "alias": alias });
        reads.ok("POST", BOB_ALIASES, body, MEMBER_ROW).await;
    }
    // The path alias is percent-decoded, trimmed and lowercased.
    let row = reads.drop_alias("%20Rob%20").await;
    assert_eq!(row["aliases"], json!(["bobby", "bob-2"]));
    let again = reads.drop_alias("rob").await;
    assert_eq!(again, row, "an alias not held leaves the row unchanged");
    let stored = reads.store.load_member("1002").await.unwrap().unwrap();
    assert_eq!(stored.aliases, ["bobby", "bob-2"]);
    // Another member's alias is not this member's to drop.
    reads.drop_alias("ali").await;
    let alice = reads.store.load_member("1001").await.unwrap().unwrap();
    assert_eq!(alice.aliases, ["ali"]);
    // Released: another member may take it.
    let path = "/api/admin/members/1001/aliases";
    let row = reads
        .ok("POST", path, json!({"alias": "rob"}), MEMBER_ROW)
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
    assert_eq!(no_csrf.error(), (403, "csrf".into()));
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
    let body = json!({"day": 0, "time": "03:00", "version": v});
    let refusal = reads.refused("POST", MOVE_KALOS, body).await;
    assert_eq!(refusal, (422, "invalid".into()));
    assert_eq!(reads.version().await, v);
    let body = json!({"day": 0, "time": "06:00", "version": v});
    let moved = reads.ok("POST", MOVE_KALOS, body, MOVE_RESULT).await;
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
    let created = reads.keyed("POST", FIXED, "c-1", create.clone()).await;
    assert_eq!(created.status, 201, "{}", created.text());
    let id = created.json()["id"].as_str().unwrap().to_owned();
    reads.drop_cara().await;
    let v = reads.version().await;
    let edit = timing(v, "22:00", "bring elixirs");
    let edited = reads.keyed("PATCH", F_KALOS, "e-1", edit.clone()).await;
    assert_eq!(edited.status, 200, "{}", edited.text());

    // Dan (on the new timing) and Bob (on f-kalos) lose the bossing role.
    for user in ["1004", "1002"] {
        reads.demote(user).await;
    }
    let replay = reads.keyed("POST", FIXED, "c-1", create.clone()).await;
    assert_eq!(replay.status, 201, "{}", replay.text());
    assert_eq!(replay.json()["id"], id.as_str());
    let replay = reads.keyed("PATCH", F_KALOS, "e-1", edit.clone()).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["note"], "bring elixirs");
    assert_eq!(reads.version().await, v + 1, "nothing applied twice");

    // A different body under a used key is still a mismatch; a fresh key is validated.
    let mut other = create.clone();
    other["time"] = "20:00".into();
    let reused = reads.keyed("POST", FIXED, "c-1", other).await;
    assert_eq!(reused.error(), (422, "idempotency_mismatch".into()));
    let fresh = reads.keyed("POST", FIXED, "c-2", create).await;
    assert_eq!(fresh.error(), (422, "invalid".into()));
}

/// A retire retry is matched on the recorded change, not on the scheduler's
/// digest (which names the materialised weeks and so moves at the reset).
#[tokio::test]
async fn retire_replays_by_record() {
    let reads = Reads::new().await;
    let key = [("Idempotency-Key", "retire-1")];
    let first = reads.call("DELETE", F_KALOS, json!({}), &key).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(first.json()["cancelled"], 2, "this and next week's runs");
    let v = reads.version().await;
    let again = reads.call("DELETE", F_KALOS, json!({}), &key).await;
    assert_eq!(again.status, 200, "{}", again.text());
    assert_valid("fixed.json#/$defs/FixedRetired", "replay", &again.json());
    assert_eq!(again.json()["cancelled"], 0);
    assert_eq!(reads.version().await, v);
    let elsewhere = reads
        .call("DELETE", "/api/admin/fixed/other", json!({}), &key)
        .await;
    assert_eq!(elsewhere.error(), (422, "idempotency_mismatch".into()));
}

impl Reads {
    async fn demote(&self, user: &str) {
        let mut profile = self.store.load_member(user).await.unwrap().unwrap();
        profile.member.has_role = false;
        self.store.put_member(profile).await.unwrap();
    }
}

/// A timing's owner is read as `owner_id` with `owner_pinned`: the first
/// participant unless staff pinned someone on create or PATCH (blamed as
/// `owner`); `""` returns a pinned timing to the default. The owner is
/// validated against the roster only when it changes and conflicts like any
/// other timing field (user decision 2026-10-09).
#[tokio::test]
async fn timing_owner_is_set_changed_validated_and_blamed() {
    let reads = Reads::new().await;
    let owner = ["owner_id", "owner", "owner_pinned"];
    let (_, row) = reads.rows().await;
    assert_eq!(fields(&row, &owner), json!(["1001", "Alice", false]));

    // Create without an owner: unpinned, the first participant owns it.
    let plain = json!({
        "weekday": 2, "time": "21:00", "bosses": "hstar", "participants": ["1002", "1001"],
        "channel_id": "kalos-four", "note": null
    });
    let created = reads.ok("POST", FIXED, plain, FIXED_ROW).await;
    assert_eq!(
        fields(&created, &["owner_id", "owner_pinned"]),
        json!(["1002", false])
    );

    // Create: an owner outside the party is pinned.
    let create = json!({
        "weekday": 3, "time": "21:00", "bosses": "hstar", "participants": ["1001", "1002"],
        "channel_id": "kalos-four", "note": null, "owner_id": "1004"
    });
    assert_valid("fixed.json#/$defs/FixedRequest", "create", &create);
    let created = reads.ok("POST", FIXED, create.clone(), FIXED_ROW).await;
    assert_eq!(fields(&created, &owner), json!(["1004", "Dan", true]));

    // Unknown, role-less, bot and malformed owners are refused, nothing written.
    reads.drop_cara().await;
    let v = reads.version().await;
    for owner in ["9999", "1003", "1005", "abc"] {
        let mut bad = create.clone();
        bad["owner_id"] = owner.into();
        let reply = reads.call("POST", FIXED, bad, &[]).await;
        assert_eq!(reply.error(), (422, "invalid".into()), "{owner}");
        assert_eq!(reply.json()["message"], "Pick an owner from the roster.");
        let mut bad = timing(v, "22:00", "bring pots");
        bad["owner_id"] = owner.into();
        assert_eq!(
            reads.refused("PATCH", F_KALOS, bad).await,
            (422, "invalid".into()),
            "{owner}"
        );
    }
    assert_eq!(reads.version().await, v);

    // Update: only the owner differs, so only `owner` is edited and blamed.
    let mut edit = timing(v, "22:00", "bring pots");
    edit["owner_id"] = "1002".into();
    let row = reads.ok("PATCH", F_KALOS, edit, FIXED_ROW).await;
    assert_eq!(row["owner_id"], "1002");
    let record = reads.store.load_change(v + 1).await.unwrap().unwrap();
    // Runs carry no owner: the timing row alone changes.
    let blamed: Vec<_> = changed_fields(&record).into_iter().collect();
    assert_eq!(
        blamed,
        [(BlameTarget::FixedRun("f-kalos".into()), "owner".to_owned())]
    );

    // A form loaded at `v` resending another owner is stale; one leaving
    // the owner out (or resending the current one) edits other fields freely.
    let mut stale = timing(v, "22:00", "bring pots");
    stale["owner_id"] = "1001".into();
    assert_eq!(
        reads.refused("PATCH", F_KALOS, stale).await,
        (409, "stale".into())
    );
    let form = timing(v, "22:00", "bring elixirs");
    let row = reads.ok("PATCH", F_KALOS, form, FIXED_ROW).await;
    let shown = ["owner_id", "note"];
    assert_eq!(fields(&row, &shown), json!(["1002", "bring elixirs"]));

    // A since-demoted current owner (outside the party) never blocks an
    // edit of other fields.
    let mut form = timing(reads.version().await, "22:00", "bring elixirs");
    form["owner_id"] = "1004".into();
    reads.ok("PATCH", F_KALOS, form, FIXED_ROW).await;
    reads.demote("1004").await;
    let mut form = timing(reads.version().await, "22:00", "bring pots");
    form["owner_id"] = "1004".into();
    let row = reads.ok("PATCH", F_KALOS, form, FIXED_ROW).await;
    assert_eq!(fields(&row, &shown), json!(["1004", "bring pots"]));

    // `""` unpins: the owner is the first participant again, blamed as `owner`.
    let v = reads.version().await;
    let mut form = timing(v, "22:00", "bring pots");
    form["owner_id"] = "".into();
    let row = reads.ok("PATCH", F_KALOS, form, FIXED_ROW).await;
    assert_eq!(
        fields(&row, &["owner_id", "owner_pinned"]),
        json!(["1001", false])
    );
    let record = reads.store.load_change(v + 1).await.unwrap().unwrap();
    assert_eq!(
        changed_fields(&record).into_iter().collect::<Vec<_>>(),
        [(BlameTarget::FixedRun("f-kalos".into()), "owner".to_owned())]
    );
}

/// An owner who lost the bossing role since the first attempt must not turn
/// a replay into a 422; a fresh key is validated as usual.
#[tokio::test]
async fn timing_owner_replays_survive_a_lost_role() {
    let reads = Reads::new().await;
    reads.drop_cara().await;
    let create = json!({
        "weekday": 3, "time": "21:00", "bosses": "hstar", "participants": ["1001"],
        "channel_id": "kalos-four", "note": null, "owner_id": "1004"
    });
    let created = reads.keyed("POST", FIXED, "own-c", create.clone()).await;
    assert_eq!(created.status, 201, "{}", created.text());
    let mut edit = timing(reads.version().await, "22:00", "bring pots");
    edit["owner_id"] = "1004".into();
    let edited = reads.keyed("PATCH", F_KALOS, "own-e", edit.clone()).await;
    assert_eq!(edited.status, 200, "{}", edited.text());
    let v = reads.version().await;

    reads.demote("1004").await;
    let replay = reads.keyed("POST", FIXED, "own-c", create.clone()).await;
    assert_eq!(replay.status, 201, "{}", replay.text());
    assert_eq!(replay.json()["id"], created.json()["id"]);
    let replay = reads.keyed("PATCH", F_KALOS, "own-e", edit).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["owner_id"], "1004");
    assert_eq!(reads.version().await, v, "nothing applied twice");

    let fresh = reads.keyed("POST", FIXED, "own-c2", create).await;
    assert_eq!(fresh.error(), (422, "invalid".into()));
}
