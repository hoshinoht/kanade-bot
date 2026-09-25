//! Calls every endpoint the PWAs use and validates each response against the
//! frozen contract in `docs/v5/api-schemas` (2xx: the endpoint's schema, else `ApiError`).

use crate::{App, assets, mock, reports, routers};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use jsonschema::{Resource, Validator};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tower::ServiceExt;

const BASE: &str = "https://kanade.invalid/api-schemas/";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn schemas() -> Vec<(String, Value)> {
    let dir = root().join("docs/v5/api-schemas");
    let mut out: Vec<(String, Value)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{dir:?}: {e}"))
        .map(|entry| entry.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .map(|p| {
            let text = fs::read_to_string(&p).unwrap();
            let schema: Value =
                serde_json::from_str(&text).unwrap_or_else(|e| panic!("{p:?}: {e}"));
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            assert_eq!(schema["$id"], format!("{BASE}{name}"), "{name}: $id");
            (name, schema)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// `target` is `file.json#/$defs/Name`.
fn validator(target: &str) -> Validator {
    jsonschema::options()
        .with_resources(
            schemas()
                .into_iter()
                .map(|(name, s)| (format!("{BASE}{name}"), Resource::from_contents(s))),
        )
        .build(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": format!("{BASE}{target}"),
        }))
        .unwrap_or_else(|e| panic!("{target}: {e}"))
}

struct Harness {
    admin: Router,
    public: Router,
    failures: Vec<String>,
    checked: usize,
}

impl Harness {
    fn new() -> Self {
        let boss_dir = root().join("web/e2e/fixtures/boss");
        let app = App {
            store: Arc::new(Mutex::new(mock::Store::new(mock::catalog::Catalog::new(
                boss_dir.clone(),
            )))),
            reports: reports::Log::default(),
            identity: assets::IdentityConfig {
                name: "Kanade".into(),
                dir: None,
            },
            knowledge: Arc::new(mock::knowledge::KnowledgeDir(root().join("boss/knowledge"))),
            public: false,
            boss_dir: Arc::new(boss_dir),
        };
        let (admin, public) = routers(app, &root().join("web"));
        Self {
            admin,
            public,
            failures: Vec::new(),
            checked: 0,
        }
    }

    async fn send(
        &self,
        public: bool,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        let body = match body {
            Some(value) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(value.to_string())
            }
            None => Body::empty(),
        };
        let router = if public { &self.public } else { &self.admin };
        let res = router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|e| panic!("{method} {path}: not JSON ({e}): {bytes:?}"));
        (status, value)
    }

    fn check(&mut self, label: &str, status: StatusCode, value: &Value, target: &str) {
        let target = if status.is_success() {
            target
        } else {
            "error.json#/$defs/ApiError"
        };
        self.checked += 1;
        let errors: Vec<String> = validator(target)
            .iter_errors(value)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        if !errors.is_empty() {
            self.failures.push(format!(
                "{label} [{status}] vs {target}:\n  {}",
                errors.join("\n  ")
            ));
        }
    }

    /// A call that must succeed and match `target`.
    async fn ok(&mut self, method: &str, path: &str, body: Option<Value>, target: &str) -> Value {
        self.expect(false, method, path, body, StatusCode::OK, target)
            .await
    }

    async fn expect(
        &mut self,
        public: bool,
        method: &str,
        path: &str,
        body: Option<Value>,
        want: StatusCode,
        target: &str,
    ) -> Value {
        let (status, value) = self.send(public, method, path, body).await;
        let label = format!(
            "{} {method} {path}",
            if public { "public" } else { "admin" }
        );
        if status != want {
            self.failures
                .push(format!("{label}: status {status}, wanted {want}: {value}"));
        }
        self.check(&label, status, &value, target);
        value
    }
}

fn s(value: &Value) -> &str {
    value.as_str().expect("string")
}

#[test]
fn every_schema_def_compiles() {
    for (name, schema) in schemas() {
        for def in schema["$defs"].as_object().expect("$defs").keys() {
            validator(&format!("{name}#/$defs/{def}"));
        }
    }
}

#[tokio::test]
async fn every_pwa_endpoint_matches_the_frozen_contract() {
    let mut h = Harness::new();

    // Both origins.
    for public in [false, true] {
        h.expect(
            public,
            "GET",
            "/api/identity",
            None,
            StatusCode::OK,
            "identity.json#/$defs/Identity",
        )
        .await;
        h.expect(public, "GET", "/api/nope", None, StatusCode::NOT_FOUND, "")
            .await;
    }
    h.expect(
        true,
        "GET",
        "/api/public/status",
        None,
        StatusCode::OK,
        "identity.json#/$defs/PublicStatus",
    )
    .await;
    for q in ["", "?week=next"] {
        h.expect(
            true,
            "GET",
            &format!("/api/public/week{q}"),
            None,
            StatusCode::OK,
            "week.json#/$defs/PublicWeek",
        )
        .await;
    }
    h.expect(
        true,
        "GET",
        "/api/admin/week",
        None,
        StatusCode::NOT_FOUND,
        "",
    )
    .await;

    // Reads.
    // Both weeks' runs; `week` ends as next week, whose runs are all still ahead.
    let mut week = Value::Null;
    let mut runs: Vec<Value> = Vec::new();
    for q in ["", "?week=next"] {
        let w = h
            .ok(
                "GET",
                &format!("/api/admin/week{q}"),
                None,
                "week.json#/$defs/Week",
            )
            .await;
        runs.extend(w["runs"].as_array().unwrap().iter().cloned());
        week = w;
        h.ok(
            "GET",
            &format!("/api/admin/stats{q}"),
            None,
            "week.json#/$defs/Stats",
        )
        .await;
    }
    h.ok(
        "GET",
        "/api/admin/summary",
        None,
        "week.json#/$defs/Summary",
    )
    .await;
    let members = h
        .ok(
            "GET",
            "/api/admin/members",
            None,
            "members.json#/$defs/MemberRows",
        )
        .await;
    h.ok(
        "GET",
        "/api/admin/personas",
        None,
        "members.json#/$defs/Personas",
    )
    .await;
    h.ok(
        "GET",
        "/api/admin/channels",
        None,
        "common.json#/$defs/Channels",
    )
    .await;
    h.ok(
        "GET",
        "/api/admin/session",
        None,
        "identity.json#/$defs/Session",
    )
    .await;
    let fixed = h
        .ok(
            "GET",
            "/api/admin/fixed",
            None,
            "fixed.json#/$defs/FixedRows",
        )
        .await;
    let bosses = h
        .ok(
            "GET",
            "/api/admin/bosses",
            None,
            "bosses.json#/$defs/BossRows",
        )
        .await;
    let events = h
        .ok(
            "GET",
            "/api/admin/bosses/events",
            None,
            "bosses.json#/$defs/EventBosses",
        )
        .await;
    let keys = bosses
        .as_array()
        .unwrap()
        .iter()
        .chain(events.as_array().unwrap())
        .map(|b| s(&b["key"]).to_owned());
    let mut knowledge = 0;
    for key in keys {
        let (status, value) = h
            .send(
                false,
                "GET",
                &format!("/api/admin/bosses/{key}/knowledge"),
                None,
            )
            .await;
        // Bosses without a tracked document answer 404.
        if status == StatusCode::OK {
            knowledge += 1;
        }
        h.check(
            &format!("GET knowledge {key}"),
            status,
            &value,
            "bosses.json#/$defs/Knowledge",
        );
    }
    assert!(knowledge > 0, "no knowledge page validated");
    h.ok(
        "GET",
        "/api/admin/reminders",
        None,
        "reminders.json#/$defs/Reminders",
    )
    .await;
    h.ok(
        "GET",
        "/api/admin/limits",
        None,
        "limits.json#/$defs/Limits",
    )
    .await;
    h.ok(
        "GET",
        "/api/admin/config",
        None,
        "config.json#/$defs/ConfigView",
    )
    .await;
    h.ok(
        "GET",
        "/api/admin/access",
        None,
        "config.json#/$defs/AccessReport",
    )
    .await;
    h.ok(
        "GET",
        "/api/admin/history/checkpoints",
        None,
        "history.json#/$defs/Checkpoints",
    )
    .await;

    for run in &runs {
        h.ok(
            "GET",
            &format!("/api/admin/runs/{}/blame", s(&run["id"])),
            None,
            "history.json#/$defs/BlameEntries",
        )
        .await;
    }

    // Logs, including their detail pages and the filter refusal.
    for q in ["", "?outcome=failed,proposed"] {
        let x = h
            .ok(
                "GET",
                &format!("/api/admin/extractions{q}"),
                None,
                "extractions.json#/$defs/Extractions",
            )
            .await;
        for row in x["rows"].as_array().unwrap() {
            h.ok(
                "GET",
                &format!("/api/admin/extractions/{}", s(&row["id"])),
                None,
                "extractions.json#/$defs/Extraction",
            )
            .await;
        }
        let c = h
            .ok(
                "GET",
                &format!(
                    "/api/admin/chat{}",
                    q.replace("failed,proposed", "answered")
                ),
                None,
                "chat.json#/$defs/Chat",
            )
            .await;
        for row in c["rows"].as_array().unwrap() {
            h.ok(
                "GET",
                &format!("/api/admin/chat/{}", s(&row["id"])),
                None,
                "chat.json#/$defs/ChatTurn",
            )
            .await;
        }
    }
    h.expect(
        false,
        "GET",
        "/api/admin/extractions?outcome=bogus",
        None,
        StatusCode::UNPROCESSABLE_ENTITY,
        "",
    )
    .await;
    h.expect(
        false,
        "GET",
        "/api/admin/chat?from=nope",
        None,
        StatusCode::UNPROCESSABLE_ENTITY,
        "",
    )
    .await;
    let targets = h
        .ok(
            "GET",
            "/api/admin/rescan/targets",
            None,
            "common.json#/$defs/Channels",
        )
        .await;
    let job = h
        .ok(
            "POST",
            "/api/admin/rescan",
            Some(json!({ "channels": [targets[0]["id"]], "window": "week" })),
            "extractions.json#/$defs/RescanJob",
        )
        .await;
    let job_path = format!("/api/admin/rescan/{}", s(&job["id"]));
    h.ok("GET", &job_path, None, "extractions.json#/$defs/RescanJob")
        .await;
    h.ok(
        "DELETE",
        &job_path,
        None,
        "extractions.json#/$defs/RescanJob",
    )
    .await;

    // Run edits.
    // A failed edit is already recorded; keep going from the last version.
    let version = |w: &Value, v: u64| w["version"].as_u64().unwrap_or(v);
    let mut v = week["version"].as_u64().unwrap();
    // The test clock is unpinned, so edit a next-week run from a timing (still ahead, resettable).
    let run = week["runs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            matches!(s(&r["status"]), "planned" | "confirmed" | "at_risk")
                && !r["fixed_id"].is_null()
                && !r["participants"].as_array().unwrap().is_empty()
        })
        .expect("a live next-week run from a timing")
        .clone();
    let run = &run;
    let id = s(&run["id"]);
    let moved = h
        .ok(
            "POST",
            &format!("/api/admin/runs/{id}/move"),
            Some(json!({ "day": run["day"], "time": "20:00", "version": v })),
            "week.json#/$defs/MoveResult",
        )
        .await;
    v = version(&moved, v);
    h.expect(
        false,
        "POST",
        &format!("/api/admin/runs/{id}/move"),
        Some(json!({ "day": 0, "time": "20:00", "version": 0 })),
        StatusCode::CONFLICT,
        "",
    )
    .await;
    let r = h
        .ok(
            "PATCH",
            &format!("/api/admin/runs/{id}/status"),
            Some(json!({ "status": "confirmed", "version": v })),
            "week.json#/$defs/RunResult",
        )
        .await;
    v = version(&r, v);
    let member = s(&run["participants"][0]["id"]);
    let r = h
        .ok(
            "POST",
            &format!("/api/admin/runs/{id}/rsvp"),
            Some(json!({ "member_id": member, "answer": "yes", "version": v })),
            "week.json#/$defs/RunResult",
        )
        .await;
    v = version(&r, v);
    let outsider = members
        .as_array()
        .unwrap()
        .iter()
        .map(|m| s(&m["id"]))
        .find(|m| {
            run["participants"]
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["id"] != *m)
        })
        .unwrap()
        .to_owned();
    for op in ["add", "remove"] {
        let r = h
            .ok(
                "PATCH",
                &format!("/api/admin/runs/{id}/participants"),
                Some(json!({ op: outsider, "version": v })),
                "week.json#/$defs/RunResult",
            )
            .await;
        v = version(&r, v);
    }
    // The move above amended it.
    let r = h
        .ok(
            "POST",
            &format!("/api/admin/runs/{id}/reset"),
            Some(json!({ "version": v })),
            "week.json#/$defs/RunResult",
        )
        .await;
    let _ = version(&r, v);
    h.ok(
        "POST",
        &format!("/api/admin/runs/{id}/ping"),
        Some(json!({})),
        "common.json#/$defs/Message",
    )
    .await;

    // Members.
    let m = s(&members[0]["id"]).to_owned();
    h.ok(
        "PATCH",
        &format!("/api/admin/members/{m}"),
        Some(json!({ "ping_level": "all", "persona": "" })),
        "members.json#/$defs/MemberRow",
    )
    .await;
    h.ok(
        "POST",
        &format!("/api/admin/members/{m}/aliases"),
        Some(json!({ "alias": "contractalias" })),
        "members.json#/$defs/MemberRow",
    )
    .await;

    // Weekly timings.
    let row = &fixed[0];
    let tokens: Vec<&str> = row["bosses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| s(&b["token"]))
        .collect();
    let participants: Vec<&str> = row["participants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| s(&p["id"]))
        .collect();
    let body = json!({
        "weekday": (row["weekday"].as_u64().unwrap() + 1) % 7,
        "time": "23:30",
        "bosses": tokens.join(" "),
        "participants": participants,
        "channel_id": row["channel_id"],
        "note": null,
    });
    h.ok(
        "POST",
        "/api/admin/validate/bosses",
        Some(json!({ "text": tokens.join(" ") })),
        "fixed.json#/$defs/ValidateResult",
    )
    .await;
    let created = h
        .ok(
            "POST",
            "/api/admin/fixed",
            Some(body.clone()),
            "fixed.json#/$defs/FixedRow",
        )
        .await;
    let fixed_path = format!("/api/admin/fixed/{}", s(&created["id"]));
    let mut edit = body;
    edit["note"] = json!("contract");
    h.ok(
        "PATCH",
        &fixed_path,
        Some(edit),
        "fixed.json#/$defs/FixedRow",
    )
    .await;
    h.ok(
        "DELETE",
        &fixed_path,
        None,
        "fixed.json#/$defs/FixedRetired",
    )
    .await;

    // Inbox: reject a member request, approve an extractor proposal.
    let inbox = h
        .ok(
            "GET",
            "/api/admin/inbox",
            None,
            "inbox.json#/$defs/Proposals",
        )
        .await;
    let items = inbox.as_array().unwrap();
    let request = items
        .iter()
        .find(|p| p["tab"] == "self_service")
        .expect("a self-service request");
    h.ok(
        "POST",
        &format!("/api/admin/inbox/{}/reject", s(&request["id"])),
        Some(json!({ "version": request["version"], "reason": "Contract test." })),
        "common.json#/$defs/Message",
    )
    .await;
    let proposal = items
        .iter()
        .find(|p| {
            p["tab"] == "extractor"
                && p["flags"].as_array().unwrap().is_empty()
                && p["choices"].is_null()
        })
        .expect("a plain extractor proposal");
    h.ok(
        "POST",
        &format!("/api/admin/inbox/{}/approve", s(&proposal["id"])),
        Some(json!({ "version": proposal["version"] })),
        "common.json#/$defs/Message",
    )
    .await;

    // Limits, config, digest, access.
    let limits = h
        .ok(
            "GET",
            "/api/admin/limits",
            None,
            "limits.json#/$defs/Limits",
        )
        .await;
    let who = s(&limits["allowances"][0]["member"]["id"]).to_owned();
    h.ok(
        "DELETE",
        &format!("/api/admin/limits/windows/{who}"),
        None,
        "common.json#/$defs/Message",
    )
    .await;
    h.ok(
        "PATCH",
        "/api/admin/config",
        Some(json!({ "notifications": { "quiet_mode": true } })),
        "config.json#/$defs/ConfigView",
    )
    .await;
    h.expect(
        false,
        "PATCH",
        "/api/admin/config",
        Some(json!({ "models": { "pii_pseudonymise": false } })),
        StatusCode::UNPROCESSABLE_ENTITY,
        "",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/config/profiles/reload",
        Some(json!({})),
        "common.json#/$defs/ReloadResult",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/digest",
        Some(json!({ "week": "this", "channel_id": null })),
        "common.json#/$defs/Message",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/access/recheck",
        Some(json!({})),
        "config.json#/$defs/AccessReport",
    )
    .await;

    // History: pages, each record, and the three rollback previews.
    let page = h
        .ok(
            "GET",
            "/api/admin/history?limit=100",
            None,
            "history.json#/$defs/HistoryPage",
        )
        .await;
    let records = page["records"].as_array().unwrap().clone();
    assert!(!records.is_empty(), "the edits above wrote history");
    for record in &records {
        h.ok(
            "GET",
            &format!("/api/admin/history/{}", record["seq"]),
            None,
            "history.json#/$defs/ChangeRecord",
        )
        .await;
    }
    let newest = &records[0];
    let week_key = s(&newest["weeks"][0]).to_owned();
    h.ok(
        "GET",
        &format!(
            "/api/admin/history?limit=2&week={week_key}&actor=admin:{}",
            s(&newest["actor"]["id"])
        ),
        None,
        "history.json#/$defs/HistoryPage",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/history/revert",
        Some(json!({ "seqs": [newest["seq"]], "preview": true })),
        "history.json#/$defs/RevertPlan",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/history/restore-week",
        Some(json!({ "week": week_key, "revision": 0, "preview": true })),
        "history.json#/$defs/RevertPlan",
    )
    .await;
    let actor = format!(
        "{}:{}",
        s(&newest["actor"]["kind"]),
        s(&newest["actor"]["id"])
    );
    h.ok(
        "POST",
        "/api/admin/history/revert-actor",
        Some(json!({ "actor": actor, "since": "1970-01-01T00:00:00Z", "preview": true })),
        "history.json#/$defs/RevertPlan",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/history/revert",
        Some(json!({ "seqs": [newest["seq"]] })),
        "history.json#/$defs/RevertPlan",
    )
    .await;
    h.expect(
        false,
        "GET",
        "/api/admin/history/999999",
        None,
        StatusCode::NOT_FOUND,
        "",
    )
    .await;

    // Closing the portal closes the public schedule.
    h.ok(
        "PATCH",
        "/api/admin/config",
        Some(json!({ "self_service": { "public_portal": false } })),
        "config.json#/$defs/ConfigView",
    )
    .await;
    let status = h
        .expect(
            true,
            "GET",
            "/api/public/status",
            None,
            StatusCode::OK,
            "identity.json#/$defs/PublicStatus",
        )
        .await;
    assert_eq!(status["portal"], "closed");
    h.expect(
        true,
        "GET",
        "/api/public/week",
        None,
        StatusCode::SERVICE_UNAVAILABLE,
        "",
    )
    .await;

    assert!(
        h.failures.is_empty(),
        "{} of {} responses broke the contract:\n{}",
        h.failures.len(),
        h.checked,
        h.failures.join("\n")
    );
}
