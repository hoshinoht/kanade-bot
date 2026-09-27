//! Calls every endpoint the PWAs use and validates each response against the
//! frozen contract in `docs/v5/api-schemas` (2xx: the endpoint's schema, else `ApiError`).

use crate::{App, assets, mock, reports, routers};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode, header},
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
    csrf: String,
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
            writes: Arc::default(),
        };
        let csrf = app.writes.token();
        let (admin, public) = routers(app, &root().join("web"));
        Self {
            admin,
            public,
            failures: Vec::new(),
            checked: 0,
            csrf,
        }
    }

    /// As the PWA sends it: with the session's CSRF token.
    async fn send(
        &self,
        public: bool,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let csrf = self.csrf.clone();
        let (status, _, value) = self
            .send_with(public, method, path, body, &[("x-kanade-csrf", &csrf)])
            .await;
        (status, value)
    }

    async fn send_with(
        &self,
        public: bool,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, HeaderMap, Value) {
        let mut req = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
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
        let headers = res.headers().clone();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        // No body (204, the sign-in redirects) or the sign-in landing page: nothing JSON to check.
        let html = headers
            .get(header::CONTENT_TYPE)
            .is_some_and(|v| v.as_bytes().starts_with(b"text/html"));
        let value = if bytes.is_empty() {
            Value::Null
        } else if html {
            Value::String(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|e| panic!("{method} {path}: not JSON ({e}): {bytes:?}"))
        };
        (status, headers, value)
    }

    /// A guarded admin call answering `want` with the ApiError `code`.
    async fn refused(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        headers: &[(&str, &str)],
        want: (StatusCode, &str),
    ) {
        let (status, _, value) = self
            .send_with(false, method, path, Some(body), headers)
            .await;
        let label = format!("admin {method} {path} ({})", want.1);
        if (status, value["error"].as_str()) != (want.0, Some(want.1)) {
            self.failures.push(format!(
                "{label}: wanted {} {}, got {status}: {value}",
                want.0, want.1
            ));
        }
        // An unexpected success is already a failure above; only refusals have a shape to check.
        if !status.is_success() {
            self.check(&label, status, &value, "");
        }
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

/// The CSRF token after a (mock) sign-in, as the PWA reads it.
async fn session_token(h: &Harness) -> String {
    let (_, headers, _) = h
        .send_with(false, "GET", "/api/admin/session", None, &[])
        .await;
    headers
        .get("x-kanade-csrf")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
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
    h.ok("GET", "/api/admin/roles", None, "common.json#/$defs/Roles")
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
    let config = h
        .ok(
            "GET",
            "/api/admin/config",
            None,
            "config.json#/$defs/ConfigView",
        )
        .await;
    let digest = s(&config["persona"]["role_profiles_digest"]).to_owned();
    let updated = h
        .ok(
            "PATCH",
            "/api/admin/config",
            Some(json!({ "persona": {
                "role_profiles": [
                    { "role_id": "300003", "profile": "sparkly" },
                    { "role_id": "300001", "profile": "terse" }
                ],
                "role_profiles_digest": digest
            } })),
            "config.json#/$defs/ConfigView",
        )
        .await;
    assert_eq!(
        updated["persona"]["role_profiles"][0]["role_name"],
        "bossers"
    );
    h.expect(
        false,
        "PATCH",
        "/api/admin/config",
        Some(json!({ "persona": {
            "role_profiles": [],
            "role_profiles_digest": digest
        } })),
        StatusCode::CONFLICT,
        "",
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
    // Edit a next-week run from a timing: still ahead and resettable on any test clock.
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
    let head = h
        .ok("GET", "/api/admin/week", None, "week.json#/$defs/Week")
        .await["version"]
        .as_u64()
        .unwrap();
    let csrf = h.csrf.clone();
    let token = [("x-kanade-csrf", csrf.as_str())];
    let mut edit = body;
    edit["note"] = json!("contract");
    h.refused(
        "PATCH",
        &fixed_path,
        edit.clone(),
        &token,
        (StatusCode::UNPROCESSABLE_ENTITY, "version_required"),
    )
    .await;
    edit["version"] = json!(head - 1);
    h.refused(
        "PATCH",
        &fixed_path,
        edit.clone(),
        &token,
        (StatusCode::CONFLICT, "stale"),
    )
    .await;
    edit["version"] = json!(head);
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

    // Write guard: CSRF on every admin write, Idempotency-Key replays.
    let (_, headers, _) = h
        .send_with(false, "GET", "/api/admin/session", None, &[])
        .await;
    assert_eq!(
        headers.get("x-kanade-csrf").and_then(|v| v.to_str().ok()),
        Some(csrf.as_str()),
        "the session carries the CSRF token"
    );
    let head = h
        .ok(
            "GET",
            "/api/admin/week?week=next",
            None,
            "week.json#/$defs/Week",
        )
        .await["version"]
        .as_u64()
        .unwrap();
    let move_path = format!("/api/admin/runs/{id}/move");
    let move_body = json!({ "day": run["day"], "time": "21:10", "version": head });
    let forbidden = (StatusCode::FORBIDDEN, "csrf");
    h.refused("POST", &move_path, move_body.clone(), &[], forbidden)
        .await;
    h.refused(
        "POST",
        &move_path,
        move_body.clone(),
        &[("x-kanade-csrf", "forged")],
        forbidden,
    )
    .await;
    h.refused(
        "POST",
        &move_path,
        move_body.clone(),
        &[("x-kanade-csrf", &csrf), ("sec-fetch-site", "cross-site")],
        forbidden,
    )
    .await;
    h.refused(
        "POST",
        &move_path,
        move_body.clone(),
        &[("x-kanade-csrf", &csrf), ("idempotency-key", "no spaces")],
        (StatusCode::BAD_REQUEST, "invalid_idempotency_key"),
    )
    .await;
    let keyed = [
        ("x-kanade-csrf", csrf.as_str()),
        ("idempotency-key", "contract:move-1"),
    ];
    let (first_status, _, first) = h
        .send_with(false, "POST", &move_path, Some(move_body.clone()), &keyed)
        .await;
    h.check(
        "keyed move",
        first_status,
        &first,
        "week.json#/$defs/MoveResult",
    );
    // The first attempt moved the version on; the retry replays instead of going stale.
    let (again_status, _, again) = h
        .send_with(false, "POST", &move_path, Some(move_body), &keyed)
        .await;
    assert_eq!((first_status, &first), (again_status, &again), "replayed");
    h.refused(
        "POST",
        &move_path,
        json!({ "day": run["day"], "time": "21:20", "version": head + 1 }),
        &keyed,
        (StatusCode::UNPROCESSABLE_ENTITY, "idempotency_mismatch"),
    )
    .await;

    // Inbox (A6): proposals from the extractor and the chatbot, member
    // requests of every type; the Discord-only rule, edits, codes, replays.
    let inbox = h
        .ok(
            "GET",
            "/api/admin/inbox",
            None,
            "inbox.json#/$defs/Proposals",
        )
        .await;
    let items = inbox.as_array().unwrap().clone();
    let item = |id: &str| {
        items
            .iter()
            .find(|p| p["id"] == id)
            .unwrap_or_else(|| panic!("{id}"))
            .clone()
    };
    for source in ["extraction", "chat", "self_service"] {
        assert!(items.iter().any(|p| p["source"] == source), "{source}");
    }
    for kind in ["new_fixed", "change_fixed", "join", "leave", "swap"] {
        assert!(items.iter().any(|p| p["kind"] == kind), "{kind}");
    }
    assert!(items.iter().any(|p| {
        p["flags"]
            .as_array()
            .unwrap()
            .contains(&json!("requester_unauthorised"))
    }));
    assert!(
        items
            .iter()
            .filter(|p| p["tab"] == "self_service")
            .all(|p| p["self_service"]["via"] == "request")
    );
    let csrf = h.csrf.clone();
    let token = [("x-kanade-csrf", csrf.as_str())];
    h.refused(
        "POST",
        "/api/admin/inbox/p-carling-link/approve",
        json!({}),
        &token,
        (StatusCode::UNPROCESSABLE_ENTITY, "version_required"),
    )
    .await;
    h.refused(
        "POST",
        "/api/admin/inbox/p-bm-move/approve",
        json!({ "force": true }),
        &token,
        (StatusCode::UNPROCESSABLE_ENTITY, "force_unsupported"),
    )
    .await;
    h.refused(
        "POST",
        "/api/admin/inbox/p-carling-link/approve",
        json!({ "version": 1, "day": 1, "time": "21:00" }),
        &token,
        (StatusCode::UNPROCESSABLE_ENTITY, "edit_not_applicable"),
    )
    .await;
    h.refused(
        "POST",
        "/api/admin/inbox/p-limbo-add/reject",
        json!({ "reason": "why" }),
        &token,
        (StatusCode::UNPROCESSABLE_ENTITY, "reason_not_applicable"),
    )
    .await;
    // A token (or Tailscale) session cannot decide Kanade's proposals.
    h.send(
        false,
        "POST",
        "/__mock/session",
        Some(json!({ "method": "token" })),
    )
    .await;
    h.csrf = session_token(&h).await;
    let csrf = h.csrf.clone();
    h.refused(
        "POST",
        "/api/admin/inbox/p-bm-move/approve",
        json!({}),
        &[("x-kanade-csrf", csrf.as_str())],
        (StatusCode::FORBIDDEN, "discord_session_required"),
    )
    .await;
    // Every session decides member requests.
    let request = item("p-fa-request");
    h.ok(
        "POST",
        "/api/admin/inbox/p-fa-request/reject",
        Some(json!({ "version": request["version"], "reason": "Contract test." })),
        "common.json#/$defs/Message",
    )
    .await;
    h.send(
        false,
        "POST",
        "/__mock/session",
        Some(json!({ "method": "discord" })),
    )
    .await;
    h.csrf = session_token(&h).await;
    // Edit then approve: one approval at a corrected time; a repeat answers 200.
    let edit = json!({ "day": 6, "time": "22:30" });
    let first = h
        .ok(
            "POST",
            "/api/admin/inbox/p-bm-move/approve",
            Some(edit.clone()),
            "common.json#/$defs/Message",
        )
        .await;
    let again = h
        .ok(
            "POST",
            "/api/admin/inbox/p-bm-move/approve",
            Some(edit),
            "common.json#/$defs/Message",
        )
        .await;
    assert_eq!(first, again, "a replayed approval answers the first result");
    let fixed_change = item("p-kalos-fixed");
    let choices: serde_json::Map<String, Value> = fixed_change["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (s(&c["run_id"]).to_owned(), json!("keep")))
        .collect();
    h.ok(
        "POST",
        "/api/admin/inbox/p-kalos-fixed/approve",
        Some(json!({ "version": fixed_change["version"], "choices": choices })),
        "common.json#/$defs/Message",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/inbox/p-jupiter-chat/approve",
        Some(json!({})),
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
    // Records name weeks by the instant they start, as the server does.
    assert!(
        week_key.len() == 25 && week_key.ends_with("+00:00"),
        "{week_key}"
    );
    assert!(
        records
            .iter()
            .flat_map(|r| r["rows"].as_array().unwrap())
            .any(|row| row["key"]["table"] == "reminders"),
        "reminder rows are recorded"
    );
    let filtered = h
        .ok(
            "GET",
            &format!(
                "/api/admin/history?limit=2&week={}&actor={}:{}",
                week_key.replace('+', "%2B"),
                s(&newest["actor"]["kind"]),
                s(&newest["actor"]["id"])
            ),
            None,
            "history.json#/$defs/HistoryPage",
        )
        .await;
    assert!(
        filtered["total"].as_u64().unwrap() > 0,
        "week filter by instant"
    );

    // Blame speaks the domain's field names.
    let blame = h
        .ok(
            "GET",
            &format!("/api/admin/runs/{id}/blame"),
            None,
            "history.json#/$defs/BlameEntries",
        )
        .await;
    let fields: Vec<&str> = blame
        .as_array()
        .unwrap()
        .iter()
        .map(|e| s(&e["field"]))
        .collect();
    assert!(
        fields.contains(&"slot") && fields.contains(&"status"),
        "{fields:?}"
    );
    assert!(
        fields.iter().all(|f| matches!(
            *f,
            "slot" | "bosses" | "participants" | "channel" | "status" | "status_pin"
        ) || f.starts_with("rsvp:")
            || f.starts_with("attended:")),
        "{fields:?}"
    );

    // A strict revert of the first move conflicts with every later edit of
    // that run: 200, no rows, the requested record named.
    let first_move = records
        .iter()
        .rev()
        .find(|r| {
            r["surface"] == "admin_portal"
                && r["rows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["key"]["id"] == id)
        })
        .expect("the run edits above")["seq"]
        .clone();
    let strict = h
        .ok(
            "POST",
            "/api/admin/history/revert",
            Some(json!({ "seqs": [first_move], "preview": true })),
            "history.json#/$defs/RevertPlan",
        )
        .await;
    assert_eq!(strict["outcome"], "conflicts", "{strict}");
    assert_eq!(strict["rows"], json!([]));
    assert_eq!(strict["reverts"], json!([first_move]));
    assert!(!strict["conflicts"].as_array().unwrap().is_empty());
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

    // Sign-in and sessions: methods, token login, sign-out, 401 while signed out, Discord.
    let (status, _, methods) = h
        .send_with(false, "GET", "/api/admin/auth/methods", None, &[])
        .await;
    assert_eq!(status, StatusCode::OK);
    for key in ["discord", "tailscale", "token"] {
        assert!(methods[key].is_boolean(), "{key}: {methods}");
    }
    let session = h
        .ok(
            "GET",
            "/api/admin/session",
            None,
            "identity.json#/$defs/Session",
        )
        .await;
    assert_eq!(session["method"], "discord");
    h.refused(
        "POST",
        "/api/admin/auth/token",
        json!({ "token": "wrong" }),
        &[],
        (StatusCode::UNAUTHORIZED, "unauthenticated"),
    )
    .await;
    h.refused(
        "POST",
        "/api/admin/auth/token",
        json!({ "tok": "x" }),
        &[],
        (StatusCode::BAD_REQUEST, "invalid_body"),
    )
    .await;
    h.refused(
        "POST",
        "/api/admin/auth/logout",
        json!({}),
        &[],
        (StatusCode::FORBIDDEN, "csrf"),
    )
    .await;
    let csrf = h.csrf.clone();
    let (status, _, _) = h
        .send_with(
            false,
            "POST",
            "/api/admin/auth/logout",
            None,
            &[("x-kanade-csrf", &csrf)],
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.expect(
        false,
        "GET",
        "/api/admin/week",
        None,
        StatusCode::UNAUTHORIZED,
        "",
    )
    .await;
    let (status, headers, session) = h
        .send_with(
            false,
            "POST",
            "/api/admin/auth/token",
            Some(json!({ "token": crate::auth::MOCK_TOKEN })),
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    h.check(
        "token login",
        status,
        &session,
        "identity.json#/$defs/Session",
    );
    assert_eq!(session["method"], "token");
    let fresh = headers
        .get("x-kanade-csrf")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        !fresh.is_empty() && fresh != csrf,
        "a sign-in issues a new CSRF token"
    );
    h.csrf = fresh;
    h.ok("GET", "/api/admin/week", None, "week.json#/$defs/Week")
        .await;
    let (status, headers, _) = h
        .send_with(
            false,
            "GET",
            "/api/admin/auth/discord/start?next=/inbox?tab=self_service",
            None,
            &[],
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let callback = headers[header::LOCATION].to_str().unwrap().to_owned();
    let (status, _, page) = h.send_with(false, "GET", &callback, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.as_str()
            .is_some_and(|p| p.contains("url=/inbox?tab=self_service")),
        "{page}"
    );
    h.send(
        false,
        "POST",
        "/__mock/discord",
        Some(json!({ "error": "forbidden" })),
    )
    .await;
    let (status, headers, _) = h
        .send_with(
            false,
            "GET",
            "/api/admin/auth/discord/start?next=//evil.example/",
            None,
            &[],
        )
        .await;
    assert_eq!(
        (status, headers[header::LOCATION].to_str().unwrap()),
        (StatusCode::SEE_OTHER, "/?login_error=forbidden")
    );

    assert!(
        h.failures.is_empty(),
        "{} of {} responses broke the contract:\n{}",
        h.failures.len(),
        h.checked,
        h.failures.join("\n")
    );
}
