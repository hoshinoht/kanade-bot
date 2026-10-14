//! Calls every endpoint the PWAs use and validates each response against the
//! frozen contract in `docs/v5/api-schemas` (2xx: the endpoint's schema, else `ApiError`).
//! A schema target is written `file#Def`, for `file.json#/$defs/Def`.

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
const NONE: &[(&str, &str)] = &[];

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

/// `target` is `file#Def`.
fn validator(target: &str) -> Validator {
    let (file, def) = target
        .split_once('#')
        .unwrap_or_else(|| panic!("{target:?}: not file#Def"));
    jsonschema::options()
        .with_resources(
            schemas()
                .into_iter()
                .map(|(name, s)| (format!("{BASE}{name}"), Resource::from_contents(s))),
        )
        .build(&json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": format!("{BASE}{file}.json#/$defs/{def}"),
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
            hints: Arc::default(),
            member_hints: Arc::default(),
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

    fn router(&self, public: bool) -> Router {
        let router = if public { &self.public } else { &self.admin };
        router.clone()
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
        let res = self
            .router(public)
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

    /// A mock control (`POST /__mock/...`) with the session's CSRF token.
    async fn control(&self, public: bool, path: &str, body: Value) -> StatusCode {
        self.send(public, "POST", path, Some(body)).await.0
    }

    /// The CSRF token after a (mock) sign-in, as the PWA reads it.
    async fn session_token(&self) -> String {
        let (_, headers, _) = self
            .send_with(false, "GET", "/api/admin/session", None, NONE)
            .await;
        header_value(&headers, "x-kanade-csrf").to_owned()
    }

    /// Signs the mock admin in again with `method` and takes the new CSRF token.
    async fn sign_in_admin(&mut self, method: &str) {
        self.control(false, "/__mock/session", json!({ "method": method }))
            .await;
        self.csrf = self.session_token().await;
    }

    /// An admin call with the session's CSRF token answering `want` with the ApiError `code`.
    async fn refused(&mut self, method: &str, path: &str, body: Value, want: (u16, &str)) {
        let csrf = self.csrf.clone();
        self.refused_with(method, path, body, &[("x-kanade-csrf", &csrf)], want)
            .await;
    }

    /// [`Self::refused`] with exactly these headers.
    async fn refused_with(
        &mut self,
        method: &str,
        path: &str,
        body: Value,
        headers: &[(&str, &str)],
        want: (u16, &str),
    ) {
        let (status, _, value) = self
            .send_with(false, method, path, Some(body), headers)
            .await;
        let label = format!("admin {method} {path} ({})", want.1);
        if status != want.0 || value["error"].as_str() != Some(want.1) {
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
            "error#ApiError"
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

    /// An image (with this session `cookie`, if any): `200`, an image type and the `cache` policy.
    async fn image(&mut self, public: bool, path: &str, cookie: &str, cache: &str) {
        let mut req = Request::get(path);
        if !cookie.is_empty() {
            req = req.header(header::COOKIE, cookie);
        }
        let res = self
            .router(public)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        self.checked += 1;
        let kind = header_value(res.headers(), "content-type");
        let got = header_value(res.headers(), "cache-control");
        if res.status() != StatusCode::OK || !kind.starts_with("image/") || got != cache {
            let origin = if public { "public" } else { "admin" };
            let status = res.status();
            self.failures
                .push(format!("{origin} GET {path}: {status} {kind} {got}"));
        }
    }

    /// A call with the session's CSRF token that must answer `want`, checked
    /// against `target` (`""` when only an error is wanted).
    async fn expect(
        &mut self,
        public: bool,
        method: &str,
        path: &str,
        body: Option<Value>,
        want: u16,
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

    /// An admin call that must succeed and match `target`.
    async fn ok(&mut self, method: &str, path: &str, body: Option<Value>, target: &str) -> Value {
        self.expect(false, method, path, body, 200, target).await
    }

    /// An admin read that must succeed and match `target`.
    async fn get(&mut self, path: &str, target: &str) -> Value {
        self.ok("GET", path, None, target).await
    }

    /// An admin call that must answer `want` with an ApiError.
    async fn fails(&mut self, method: &str, path: &str, body: Option<Value>, want: u16) -> Value {
        self.expect(false, method, path, body, want, "").await
    }

    /// `PATCH /api/admin/config` with `patch`; the new view.
    async fn config(&mut self, patch: Value) -> Value {
        self.ok(
            "PATCH",
            "/api/admin/config",
            Some(patch),
            "config#ConfigView",
        )
        .await
    }

    /// A public-origin call with these headers; `target` is checked on 2xx with a body.
    async fn public(
        &mut self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        want: u16,
        target: &str,
    ) -> (HeaderMap, Value) {
        self.public_with(method, path, None, headers, want, target)
            .await
    }

    /// [`Self::public`] with a JSON body.
    async fn public_with(
        &mut self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
        want: u16,
        target: &str,
    ) -> (HeaderMap, Value) {
        let (status, sent, value) = self.send_with(true, method, path, body, headers).await;
        let label = format!("public {method} {path}");
        if status != want {
            self.failures
                .push(format!("{label}: status {status}, wanted {want}: {value}"));
        }
        if !value.is_null() && !value.is_string() {
            self.check(&label, status, &value, target);
        }
        (sent, value)
    }

    /// The mock member's session (`POST /__mock/public/sign-in`): its cookie and CSRF token.
    async fn sign_in_public(&mut self) -> (String, String) {
        let (headers, _) = self
            .public("POST", "/__mock/public/sign-in", NONE, 204, "")
            .await;
        let token = header_value(&headers, "x-kanade-csrf").to_owned();
        (session_cookie(&headers), token)
    }

    /// Records a failure unless `ok`, counted as one more check.
    fn verify(&mut self, ok: bool, failure: impl FnOnce() -> String) {
        self.checked += 1;
        if !ok {
            self.failures.push(failure());
        }
    }

    fn finish(self) {
        assert!(
            self.failures.is_empty(),
            "{} of {} responses broke the contract:\n{}",
            self.failures.len(),
            self.checked,
            self.failures.join("\n")
        );
    }
}

fn s(value: &Value) -> &str {
    value.as_str().expect("string")
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

/// The `kanade_pub=<id>` pair a `Set-Cookie` sets (empty when it clears it).
fn session_cookie(headers: &HeaderMap) -> String {
    let line = header_value(headers, "set-cookie");
    line.split(';').next().unwrap_or_default().to_owned()
}

/// A member write's headers: the session cookie, its token and `key`.
fn signed<'a>(cookie: &'a str, token: &'a str, key: &'a str) -> [(&'a str, &'a str); 3] {
    [
        ("cookie", cookie),
        ("x-kanade-csrf", token),
        ("idempotency-key", key),
    ]
}

#[test]
fn every_schema_def_compiles() {
    for (name, schema) in schemas() {
        let file = name.trim_end_matches(".json");
        for def in schema["$defs"].as_object().expect("$defs").keys() {
            validator(&format!("{file}#{def}"));
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
            200,
            "identity#Identity",
        )
        .await;
        h.expect(public, "GET", "/api/nope", None, 404, "").await;
    }
    h.expect(
        true,
        "GET",
        "/api/public/status",
        None,
        200,
        "public#PublicStatus",
    )
    .await;
    h.expect(true, "GET", "/api/admin/week", None, 404, "")
        .await;

    // Reads.
    // Both weeks; `week` ends as next week, whose runs are all still ahead.
    let mut week = Value::Null;
    for q in ["", "?week=next"] {
        week = h.get(&format!("/api/admin/week{q}"), "week#Week").await;
    }
    let summary = h.get("/api/admin/summary", "week#Summary").await;
    assert_eq!(summary["quiet_mode"], false, "quiet mode starts off");
    let members = h.get("/api/admin/members", "members#MemberRows").await;
    for (path, target) in [
        ("/api/admin/personas", "members#Personas"),
        ("/api/admin/channels", "common#Channels"),
        ("/api/admin/roles", "common#Roles"),
        ("/api/admin/session", "identity#Session"),
    ] {
        h.get(path, target).await;
    }
    let me = h.get("/api/admin/me", "identity#Me").await;
    assert_eq!(me["member"]["id"], "1001");
    // Asahi holds `staff`, whose role assignment (Terse) beats the saved Kanade.
    assert_eq!(me["member"]["reply_style"]["source"], "role");
    assert_eq!(me["member"]["reply_style"]["role_name"], "staff");
    assert_eq!(me["member"]["reply_style"]["saved"]["key"], "kanade");
    let personas = h.get("/api/admin/personas", "members#Personas").await;
    assert!(personas[0]["voice"].is_string());

    // Account → Sessions: list, end another, refuse this one, end the rest.
    let sessions = h
        .get("/api/admin/me/sessions", "identity#AccountSessions")
        .await;
    let handle = |current: bool| {
        sessions["sessions"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["current"] == current))
            .map(|row| s(&row["handle"]).to_owned())
            .unwrap_or_default()
    };
    let (own, other) = (handle(true), handle(false));
    let other = format!("/api/admin/me/sessions/{other}");
    let (status, _) = h.send(false, "DELETE", &other, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.fails("DELETE", &other, None, 404).await;
    h.fails(
        "DELETE",
        &format!("/api/admin/me/sessions/{own}"),
        None,
        409,
    )
    .await;
    let ended = h
        .ok(
            "POST",
            "/api/admin/me/sessions/sign-out-others",
            None,
            "identity#SessionsEnded",
        )
        .await;
    assert_eq!(ended["ended"], 1);
    let fixed = h.get("/api/admin/fixed", "fixed#FixedRows").await;
    let bosses = h.get("/api/admin/bosses", "bosses#BossRows").await;
    let events = h
        .get("/api/admin/bosses/events", "bosses#EventBosses")
        .await;
    let keys = bosses
        .as_array()
        .unwrap()
        .iter()
        .chain(events.as_array().unwrap())
        .map(|b| s(&b["key"]).to_owned());
    let mut knowledge = 0;
    for key in keys {
        let path = format!("/api/admin/bosses/{key}/knowledge");
        let (status, value) = h.send(false, "GET", &path, None).await;
        // Bosses without a tracked document answer 404.
        if status == StatusCode::OK {
            knowledge += 1;
        }
        h.check(
            &format!("GET knowledge {key}"),
            status,
            &value,
            "bosses#Knowledge",
        );
    }
    assert!(knowledge > 0, "no knowledge page validated");
    let reminders = h.get("/api/admin/reminders", "reminders#Reminders").await;
    let preview_path = |row: &Value| format!("/api/admin/reminders/{}/preview", s(&row["id"]));
    // Every row's card preview, queued and sent.
    for list in ["upcoming", "sent"] {
        for row in reminders[list].as_array().unwrap() {
            let shown = h.get(&preview_path(row), "reminders#ReminderPreview").await;
            assert_eq!(shown["reminder"]["id"], row["id"]);
        }
    }
    // The redesigned style: every preview still matches the schema, and a
    // morning card shows one embed per run with inline answer fields.
    let styled = h
        .config(json!({ "notifications": { "message_style": "redesigned" } }))
        .await;
    assert_eq!(styled["notifications"]["message_style"], "redesigned");
    let mut mornings = 0;
    for list in ["upcoming", "sent"] {
        for row in reminders[list].as_array().unwrap() {
            let shown = h.get(&preview_path(row), "reminders#ReminderPreview").await;
            let card = &shown["card"];
            if row["kind"] == "morning" && !card.is_null() {
                mornings += 1;
                assert!(s(&card["content"]).contains("\n-# "), "{card}");
                assert_eq!(card["fields"][0]["inline"], true, "{card}");
                assert!(card["title"].is_string(), "{card}");
            }
        }
    }
    assert!(mornings > 0, "no redesigned morning card validated");
    let config = "/api/admin/config";
    h.fails(
        "PATCH",
        config,
        Some(json!({ "notifications": { "message_style": "fancy" } })),
        422,
    )
    .await;
    h.config(json!({ "notifications": { "message_style": "classic" } }))
        .await;
    let timed = h
        .config(json!({ "notifications": { "header_generation_time": "03:30" } }))
        .await;
    assert_eq!(timed["notifications"]["header_generation_time"], "03:30");
    h.fails(
        "PATCH",
        config,
        Some(json!({ "notifications": { "header_generation_time": "25:00" } })),
        422,
    )
    .await;
    h.fails("GET", "/api/admin/reminders/nope/preview", None, 404)
        .await;
    h.get("/api/admin/limits", "limits#Limits").await;
    let view = h.get(config, "config#ConfigView").await;
    let digest = s(&view["persona"]["role_profiles_digest"]).to_owned();
    let updated = h
        .config(json!({ "persona": {
            "role_profiles": [
                { "role_id": "300003", "profile": "sparkly" },
                { "role_id": "300001", "profile": "terse" }
            ],
            "role_profiles_digest": digest
        } }))
        .await;
    assert_eq!(
        updated["persona"]["role_profiles"][0]["role_name"],
        "bossers"
    );
    h.fails(
        "PATCH",
        config,
        Some(json!({ "persona": {
            "role_profiles": [],
            "role_profiles_digest": digest
        } })),
        409,
    )
    .await;
    h.get("/api/admin/access", "config#AccessReport").await;
    h.get("/api/admin/history/checkpoints", "history#Checkpoints")
        .await;
    for q in ["", "?realm=member&event=login_succeeded"] {
        h.get(
            &format!("/api/admin/history/sign-ins{q}"),
            "history#SignInPage",
        )
        .await;
    }

    // Logs, including their detail pages and the filter refusal.
    for q in ["", "?outcome=failed,proposed"] {
        let x = h
            .get(
                &format!("/api/admin/extractions{q}"),
                "extractions#Extractions",
            )
            .await;
        for row in x["rows"].as_array().unwrap() {
            let path = format!("/api/admin/extractions/{}", s(&row["id"]));
            h.get(&path, "extractions#Extraction").await;
        }
        let q = q.replace("failed,proposed", "answered");
        let c = h.get(&format!("/api/admin/chat{q}"), "chat#Chat").await;
        for row in c["rows"].as_array().unwrap() {
            h.get(
                &format!("/api/admin/chat/{}", s(&row["id"])),
                "chat#ChatTurn",
            )
            .await;
        }
    }
    for q in [
        "",
        "?stage=batch&verdict=unavailable,accepted",
        "?kind=nudge",
    ] {
        let r = h
            .get(&format!("/api/admin/rewrites{q}"), "rewrites#Rewrites")
            .await;
        for row in r["rows"].as_array().unwrap() {
            let path = format!("/api/admin/rewrites/{}", s(&row["id"]));
            h.get(&path, "rewrites#Rewrite").await;
        }
    }
    for bad in [
        "?verdict=bogus",
        "?kind=send",
        "?outcome=accepted",
        "?from=2026-02-30",
        "?stage=batch&stage=debug",
        "?q=%ZZ",
    ] {
        h.fails("GET", &format!("/api/admin/rewrites{bad}"), None, 422)
            .await;
    }
    for (path, want) in [
        ("/api/admin/rewrites/rw-missing", 404),
        ("/api/admin/extractions?outcome=bogus", 422),
        ("/api/admin/chat?from=nope", 422),
    ] {
        h.fails("GET", path, None, want).await;
    }
    let targets = h.get("/api/admin/rescan/targets", "common#Channels").await;
    let job = h
        .ok(
            "POST",
            "/api/admin/rescan",
            Some(json!({ "channels": [targets[0]["id"]], "window": "week" })),
            "extractions#RescanJob",
        )
        .await;
    let job_path = format!("/api/admin/rescan/{}", s(&job["id"]));
    h.get(&job_path, "extractions#RescanJob").await;
    h.ok("DELETE", &job_path, None, "extractions#RescanJob")
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
    let run_path = |action: &str| format!("/api/admin/runs/{id}/{action}");
    let moved = h
        .ok(
            "POST",
            &run_path("move"),
            Some(json!({ "day": run["day"], "time": "20:00", "version": v })),
            "week#MoveResult",
        )
        .await;
    v = version(&moved, v);
    h.fails(
        "POST",
        &run_path("move"),
        Some(json!({ "day": 0, "time": "20:00", "version": 0 })),
        409,
    )
    .await;
    let r = h
        .ok(
            "PATCH",
            &run_path("status"),
            Some(json!({ "status": "confirmed", "version": v })),
            "week#RunResult",
        )
        .await;
    v = version(&r, v);
    let member = s(&run["participants"][0]["id"]);
    let r = h
        .ok(
            "POST",
            &run_path("rsvp"),
            Some(json!({ "member_id": member, "answer": "yes", "version": v })),
            "week#RunResult",
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
        let body = json!({ op: outsider, "version": v });
        let r = h
            .ok(
                "PATCH",
                &run_path("participants"),
                Some(body),
                "week#RunResult",
            )
            .await;
        v = version(&r, v);
    }
    // The move above amended it.
    let body = json!({ "version": v });
    h.ok("POST", &run_path("reset"), Some(body), "week#RunResult")
        .await;
    h.ok("POST", &run_path("ping"), Some(json!({})), "common#Message")
        .await;

    // A planner swap changes both next-week rows under one version.
    let swap_week = h.get("/api/admin/week?week=next", "week#Week").await;
    let swap_runs: Vec<&Value> = swap_week["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|run| !matches!(s(&run["status"]), "done" | "cancelled"))
        .take(2)
        .collect();
    let swap_path = format!("/api/admin/runs/{}/swap", s(&swap_runs[0]["id"]));
    let mut swap = json!({
        "with": s(&swap_runs[1]["id"]),
        "version": swap_week["version"],
    });
    h.ok("POST", &swap_path, Some(swap.clone()), "week#SwapResult")
        .await;
    swap["unexpected"] = json!(true);
    h.refused("POST", &swap_path, swap, (400, "invalid_body"))
        .await;

    // Members.
    let m = s(&members[0]["id"]).to_owned();
    h.ok(
        "PATCH",
        &format!("/api/admin/members/{m}"),
        Some(json!({ "ping_level": "all", "persona": "" })),
        "members#MemberRow",
    )
    .await;
    h.ok(
        "POST",
        &format!("/api/admin/members/{m}/aliases"),
        Some(json!({ "alias": "contractalias" })),
        "members#MemberRow",
    )
    .await;
    for _ in 0..2 {
        let path = format!("/api/admin/members/{m}/aliases/%20ContractAlias");
        let row = h.ok("DELETE", &path, None, "members#MemberRow").await;
        assert!(
            !row["aliases"]
                .as_array()
                .unwrap()
                .iter()
                .any(|a| a == "contractalias"),
            "removed, and removing again is a no-op"
        );
    }

    // Roster characters (module roster): CRUD and the main rules, on Ren
    // #1013, who has no seeded characters.
    let chars = "/api/admin/members/1013/characters";
    let created = "characters#CharacterCreated";
    let body = json!({ "ign": "Wisteria", "role": "alt", "level": 260 });
    let first = h
        .expect(false, "POST", chars, Some(body), 201, created)
        .await;
    assert_eq!(first["role"], "main", "the first character is the main");
    assert_eq!(first["link"], "pending", "a MapleSEA create asks to link");
    let body = json!({ "ign": "Heather" });
    let second = h
        .expect(false, "POST", chars, Some(body), 201, created)
        .await;
    let first_path = format!("/api/admin/characters/{}", s(&first["id"]));
    h.refused("DELETE", &first_path, json!({}), (409, "main_required"))
        .await;
    h.refused(
        "POST",
        chars,
        json!({ "ign": "WISTERIA" }),
        (422, "ign_taken"),
    )
    .await;
    let promoted = h
        .ok(
            "PATCH",
            &format!("/api/admin/characters/{}", s(&second["id"])),
            Some(json!({ "version": 1, "role": "main" })),
            "characters#CharacterRow",
        )
        .await;
    assert_eq!(promoted["role"], "main");
    let listed = h.get(chars, "characters#MemberCharacters").await;
    assert_eq!(listed["characters"][0]["role"], "alt", "the crown moved");
    h.ok("DELETE", &first_path, None, "common#Message").await;

    // Run character picks (module roster): the seeded boards on r-carling.
    let run_chars = h
        .get(
            "/api/admin/runs/r-carling/characters",
            "characters#RunCharacters",
        )
        .await;
    let ren = run_chars["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["member_id"] == "1002")
        .unwrap();
    assert_eq!(ren["warnings"][0]["run_id"], "r-nstar");
    let pick = |id: Value, version: u64| json!({ "character_id": id, "version": version });
    let slot_row = "characters#RunCharacterSlot";
    let slot = "/api/admin/runs/r-carling/characters/1003";
    let picked = h
        .ok("PUT", slot, Some(pick(json!("ch-s2"), 0)), slot_row)
        .await;
    assert_eq!(picked["picked"], "ch-s2");
    h.refused("PUT", slot, pick(Value::Null, 0), (409, "stale"))
        .await;
    h.ok("PUT", slot, Some(pick(Value::Null, 1)), slot_row)
        .await;

    // Run readiness (module character_data).
    let ready = h
        .get(
            "/api/admin/runs/r-carling/readiness",
            "character_data#RunReadiness",
        )
        .await;
    assert_eq!(ready["nexon_data"], true);

    // Fixed readiness and timing picks (modules roster and character_data).
    let timing = h
        .get(
            "/api/admin/fixed/f-carling/characters",
            "characters#TimingCharacters",
        )
        .await;
    assert_eq!(timing["fixed_run_id"], "f-carling");
    let slot = "/api/admin/fixed/f-carling/characters/1003";
    let mika = h
        .ok("PUT", slot, Some(pick(json!("ch-s4"), 1)), slot_row)
        .await;
    assert_eq!(
        (&mika["picked"], &mika["basis"]),
        (&json!("ch-s4"), &json!("timing_pick"))
    );
    h.refused("PUT", slot, pick(Value::Null, 1), (409, "stale"))
        .await;
    h.get(
        "/api/admin/fixed/f-carling/readiness",
        "character_data#TimingReadiness",
    )
    .await;
    let preview = h
        .ok(
            "POST",
            "/api/admin/fixed/readiness",
            Some(json!({ "bosses": ["HCarling"], "participants": ["1003", "1008"] })),
            "character_data#TimingReadiness",
        )
        .await;
    assert_eq!(preview["fixed_run_id"], Value::Null);

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
        "fixed#ValidateResult",
    )
    .await;
    let created = h
        .ok(
            "POST",
            "/api/admin/fixed",
            Some(body.clone()),
            "fixed#FixedRow",
        )
        .await;
    let fixed_path = format!("/api/admin/fixed/{}", s(&created["id"]));
    let head = h.get("/api/admin/week", "week#Week").await["version"]
        .as_u64()
        .unwrap();
    let mut edit = body;
    edit["note"] = json!("contract");
    h.refused(
        "PATCH",
        &fixed_path,
        edit.clone(),
        (422, "version_required"),
    )
    .await;
    edit["version"] = json!(head - 1);
    h.refused("PATCH", &fixed_path, edit.clone(), (409, "stale"))
        .await;
    edit["version"] = json!(head);
    // Owner: defaults to the first participant; a role-less member is refused.
    assert_eq!(created["owner_id"], json!(participants[0]));
    let mut unrostered = edit.clone();
    unrostered["owner_id"] = json!("1014");
    h.refused("PATCH", &fixed_path, unrostered, (422, "invalid"))
        .await;
    edit["owner_id"] = json!("1012");
    let edited = h
        .ok("PATCH", &fixed_path, Some(edit), "fixed#FixedRow")
        .await;
    assert_eq!(
        (&edited["owner_id"], &edited["owner"]),
        (&json!("1012"), &json!("Minato"))
    );
    h.ok("DELETE", &fixed_path, None, "fixed#FixedRetired")
        .await;

    // Write guard: CSRF on every admin write, Idempotency-Key replays.
    let csrf = h.csrf.clone();
    assert_eq!(
        h.session_token().await,
        csrf,
        "the session carries the CSRF token"
    );
    let head = h.get("/api/admin/week?week=next", "week#Week").await["version"]
        .as_u64()
        .unwrap();
    let move_path = run_path("move");
    let move_body = json!({ "day": run["day"], "time": "21:10", "version": head });
    let unmarked: [&[(&str, &str)]; 3] = [
        NONE,
        &[("x-kanade-csrf", "forged")],
        &[("x-kanade-csrf", &csrf), ("sec-fetch-site", "cross-site")],
    ];
    for headers in unmarked {
        h.refused_with(
            "POST",
            &move_path,
            move_body.clone(),
            headers,
            (403, "csrf"),
        )
        .await;
    }
    h.refused_with(
        "POST",
        &move_path,
        move_body.clone(),
        &[("x-kanade-csrf", &csrf), ("idempotency-key", "no spaces")],
        (400, "invalid_idempotency_key"),
    )
    .await;
    let keyed = [
        ("x-kanade-csrf", csrf.as_str()),
        ("idempotency-key", "contract:move-1"),
    ];
    let (first_status, _, first) = h
        .send_with(false, "POST", &move_path, Some(move_body.clone()), &keyed)
        .await;
    h.check("keyed move", first_status, &first, "week#MoveResult");
    // The first attempt moved the version on; the retry replays instead of going stale.
    let (again_status, _, again) = h
        .send_with(false, "POST", &move_path, Some(move_body), &keyed)
        .await;
    assert_eq!((first_status, &first), (again_status, &again), "replayed");
    h.refused_with(
        "POST",
        &move_path,
        json!({ "day": run["day"], "time": "21:20", "version": head + 1 }),
        &keyed,
        (422, "idempotency_mismatch"),
    )
    .await;

    // Inbox (A6): proposals from the extractor and the chatbot, member
    // requests of every type; the Discord-only rule, edits, codes, replays.
    let inbox = h.get("/api/admin/inbox", "inbox#Proposals").await;
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
    let used: Vec<Value> = item("p-bm-move")["thread"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["used"].clone())
        .collect();
    assert!(used.contains(&json!(true)) && used.contains(&json!(false)));
    for id in ["p-carling-link", "p-fa-request"] {
        assert_eq!(item(id)["thread"], Value::Null, "{id}");
    }
    for kind in ["new_fixed", "change_fixed", "join", "leave", "swap"] {
        assert!(items.iter().any(|p| p["kind"] == kind), "{kind}");
    }
    assert_eq!(
        item("p-bm-move")["consequence"],
        "Party unchanged · 3 reminders will move"
    );
    assert_eq!(item("p-fa-request")["consequence"], Value::Null);
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

    // Past: closed items, newest first, every outcome, paged by the last id.
    let past = h.get("/api/admin/inbox/past", "inbox#PastPage").await;
    let closed = past["items"].as_array().unwrap().clone();
    assert_eq!(past["next_before"], Value::Null);
    for outcome in [
        "approved",
        "rejected",
        "superseded",
        "discarded",
        "withdrawn",
        "expired",
    ] {
        assert!(closed.iter().any(|p| p["outcome"] == outcome), "{outcome}");
    }
    for tab in ["extractor", "self_service"] {
        assert!(closed.iter().any(|p| p["tab"] == tab), "{tab}");
    }
    let first = h
        .get("/api/admin/inbox/past?limit=4", "inbox#PastPage")
        .await;
    let cursor = s(&first["next_before"]).to_owned();
    assert_eq!(first["items"][3]["id"], cursor.as_str());
    let second = h
        .get(
            &format!("/api/admin/inbox/past?limit=4&before={cursor}"),
            "inbox#PastPage",
        )
        .await;
    assert_eq!(second["items"][0], closed[4]);
    for bad in ["?limit=0", "?limit=201", "?before=nope"] {
        let path = format!("/api/admin/inbox/past{bad}");
        h.refused_with("GET", &path, json!({}), NONE, (422, "invalid_query"))
            .await;
    }
    h.expect(true, "GET", "/api/admin/inbox/past", None, 404, "")
        .await;
    let decide = |id: &str, verdict: &str| format!("/api/admin/inbox/{id}/{verdict}");
    for (id, verdict, body, code) in [
        ("p-carling-link", "approve", json!({}), "version_required"),
        (
            "p-bm-move",
            "approve",
            json!({ "force": true }),
            "force_unsupported",
        ),
        (
            "p-carling-link",
            "approve",
            json!({ "version": 1, "day": 1, "time": "21:00" }),
            "edit_not_applicable",
        ),
        (
            "p-limbo-add",
            "reject",
            json!({ "reason": "why" }),
            "reason_not_applicable",
        ),
    ] {
        h.refused("POST", &decide(id, verdict), body, (422, code))
            .await;
    }
    // A token (or Tailscale) session cannot decide Kanade's proposals.
    h.sign_in_admin("token").await;
    let bm_move = decide("p-bm-move", "approve");
    h.refused(
        "POST",
        &bm_move,
        json!({}),
        (403, "discord_session_required"),
    )
    .await;
    // Every session decides member requests.
    let request = item("p-fa-request");
    h.ok(
        "POST",
        &decide("p-fa-request", "reject"),
        Some(json!({ "version": request["version"], "reason": "Contract test." })),
        "common#Message",
    )
    .await;
    h.sign_in_admin("discord").await;
    // Edit then approve: one approval at a corrected time; a repeat answers 200.
    let edit = json!({ "day": 6, "time": "22:30" });
    let first = h
        .ok("POST", &bm_move, Some(edit.clone()), "common#Message")
        .await;
    let again = h.ok("POST", &bm_move, Some(edit), "common#Message").await;
    assert_eq!(first, again, "a replayed approval answers the first result");
    let fixed_change = item("p-kalos-fixed");
    let choices: serde_json::Map<String, Value> = fixed_change["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (s(&c["run_id"]).to_owned(), json!("keep")))
        .collect();
    let requested: Vec<&str> = fixed_change["requested"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| s(&c["id"]))
        .collect();
    assert_eq!(requested, ["0.day_time", "0.party"]);
    let edit = json!({
        "retime": [{"change": "0.day_time", "day": 5, "time": "22:00"}],
        "drop": ["0.party"],
    });
    let kalos = decide("p-kalos-fixed", "approve");
    h.refused(
        "POST",
        &kalos,
        json!({ "version": fixed_change["version"], "choices": choices, "edit": {"drop": ["0.day_time", "0.party"]} }),
        (422, "nothing_approved"),
    )
    .await;
    let approved = h
        .ok(
            "POST",
            &kalos,
            Some(json!({ "version": fixed_change["version"], "choices": choices, "edit": edit })),
            "common#Message",
        )
        .await;
    assert!(s(&approved["message"]).starts_with("Approved with changes"));

    // Drafts: list, detail, preview, writes, conflicts and merge.
    let open = h.get("/api/admin/drafts", "drafts#DraftList").await;
    assert_eq!(open["drafts"].as_array().unwrap().len(), 2);
    h.get("/api/admin/drafts?state=closed", "drafts#DraftList")
        .await;
    let graph = h
        .get(
            "/api/admin/drafts/graph?from=1&to=1000",
            "drafts#DraftGraph",
        )
        .await;
    assert_eq!(graph["drafts"].as_array().unwrap().len(), 4);
    h.refused_with(
        "GET",
        "/api/admin/drafts/graph?from=2&to=1",
        json!(null),
        NONE,
        (422, "invalid_query"),
    )
    .await;
    // Presence: the boards' simulated admins, never the caller.
    h.send(false, "POST", "/__mock/presence/board", None).await;
    let presence = "/api/admin/presence";
    let here = h
        .ok(
            "PUT",
            presence,
            Some(json!({"at": {"kind": "request", "id": "p-kalos-fixed"}, "mode": "editing", "seq": 1})),
            "presence#PresenceHere",
        )
        .await;
    assert_eq!(here["here"].as_array().unwrap().len(), 2);
    let run = h
        .get(
            "/api/admin/presence?at=run%3Ar-carling",
            "presence#PresenceList",
        )
        .await;
    assert_eq!(run["here"][0]["admin"]["name"], "Ren");
    let (left, _) = h
        .send(
            false,
            "DELETE",
            "/api/admin/presence?at=run%3Ar-carling&seq=2",
            None,
        )
        .await;
    assert_eq!(left, StatusCode::NO_CONTENT);
    h.refused(
        "PUT",
        presence,
        json!({"at": {"kind": "draft", "id": "d-weekend"}, "mode": "viewing", "seq": 1}),
        (400, "invalid_body"),
    )
    .await;
    h.refused_with(
        "PUT",
        presence,
        json!({"at": {"kind": "run", "id": "r-carling"}, "mode": "viewing", "seq": 1}),
        NONE,
        (403, "csrf"),
    )
    .await;
    h.refused_with(
        "GET",
        "/api/admin/presence?at=run:",
        json!(null),
        NONE,
        (422, "invalid_query"),
    )
    .await;
    // The seq is required on the check-in and on the leave.
    h.refused(
        "PUT",
        presence,
        json!({"at": {"kind": "run", "id": "r-carling"}, "mode": "viewing"}),
        (400, "invalid_body"),
    )
    .await;
    h.refused(
        "DELETE",
        "/api/admin/presence?at=run%3Ar-carling",
        json!(null),
        (422, "invalid_query"),
    )
    .await;
    h.send(false, "DELETE", "/__mock/presence", None).await;
    h.get("/api/admin/drafts/d-weekend", "drafts#DraftDetail")
        .await;
    let carling = "/api/admin/drafts/d-carling";
    let conflicted = h
        .get(&format!("{carling}/preview"), "drafts#DraftPreview")
        .await;
    assert_eq!(conflicted["conflicts"].as_array().unwrap().len(), 2);
    assert_eq!(conflicted["mergeable"], json!(false));
    let v4 = json!({"version": 4});
    h.refused(
        "POST",
        &format!("{carling}/merge"),
        v4.clone(),
        (409, "base_moved"),
    )
    .await;
    h.refused("POST", &format!("{carling}/rebase"), v4, (409, "conflicts"))
        .await;
    let draft = "drafts#DraftWritten";
    h.ok(
        "POST",
        &format!("{carling}/rebase"),
        Some(json!({"version": 4, "settle": {
            "both/run:r-carling/slot": "take",
            "removed/run:r-limbo": "keep",
        }})),
        draft,
    )
    .await;
    let rebased = h
        .get(&format!("{carling}/preview"), "drafts#DraftPreview")
        .await;
    assert_eq!(rebased["mergeable"], json!(true));
    let body = json!({"version": 5});
    let merged = h
        .ok("POST", &format!("{carling}/merge"), Some(body), draft)
        .await;
    assert_eq!(merged["status"], json!("merged"));
    h.refused(
        "POST",
        "/api/admin/drafts/d-weekend/ops",
        json!({"version": 1, "op": {"kind": "reset", "run": "r-kalos"}}),
        (409, "stale"),
    )
    .await;
    let body = json!({"title": "Contract"});
    let created = h
        .expect(false, "POST", "/api/admin/drafts", Some(body), 201, draft)
        .await;
    let new = format!("/api/admin/drafts/{}", s(&created["id"]));
    h.ok(
        "POST",
        &format!("{new}/ops"),
        Some(json!({"version": 1, "op": {"kind": "status", "run": "r-kalos", "status": "confirmed"}})),
        draft,
    )
    .await;
    let body = json!({"version": 2, "title": "Contract check"});
    h.ok("PATCH", &new, Some(body), draft).await;
    h.ok("DELETE", &format!("{new}/ops/0?version=3"), None, draft)
        .await;
    h.ok("DELETE", &format!("{new}?version=4"), None, draft)
        .await;
    h.ok(
        "POST",
        &decide("p-jupiter-chat", "approve"),
        Some(json!({})),
        "common#Message",
    )
    .await;

    // Limits, config, digest, access.
    let limits = h.get("/api/admin/limits", "limits#Limits").await;
    // A window with answers in it (staff are exempt, so never counted).
    let counted = limits["allowances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["used"].as_u64() > Some(0))
        .unwrap();
    assert!(counted["resets_at"].as_str().is_some(), "{counted}");
    let who = s(&counted["member"]["id"]).to_owned();
    let path = format!("/api/admin/limits/windows/{who}");
    h.ok("DELETE", &path, None, "common#Message").await;
    // The clear is a `limits` row in History, in the settings stream's shape.
    let cleared = h.get("/api/admin/history", "history#HistoryPage").await;
    assert!(
        cleared["settings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["section"] == "limits"
                && row["values"][0]["key"] == format!("window.{who}")),
        "{cleared}"
    );
    // Portraits are images, never JSON or HTML.
    for path in [
        "/api/admin/members/1001/avatar",
        "/api/admin/members/1005/avatar",
        "/api/admin/me/avatar",
    ] {
        h.image(false, path, "", "private, no-cache").await;
    }
    h.config(json!({ "notifications": { "quiet_mode": true } }))
        .await;
    let quiet = h.get("/api/admin/summary", "week#Summary").await;
    assert_eq!(quiet["quiet_mode"], true, "the summary follows the config");
    let lengths = h
        .config(json!({ "run_lengths": { "default_minutes": 20 } }))
        .await;
    assert_eq!(lengths["run_lengths"]["default_minutes"], 20);
    let profanity = h
        .config(json!({ "profanity": { "extra_words": ["heck"], "check_replies": false } }))
        .await;
    assert_eq!(profanity["profanity"]["extra_words"], json!(["heck"]));
    let body = json!({ "models": { "pii_pseudonymise": false } });
    h.fails("PATCH", config, Some(body), 422).await;
    h.ok(
        "POST",
        "/api/admin/config/profiles/reload",
        Some(json!({})),
        "common#ReloadResult",
    )
    .await;
    h.ok(
        "POST",
        "/api/admin/digest",
        Some(json!({ "week": "this", "channel_id": null })),
        "common#Message",
    )
    .await;
    let rewrite = "/api/admin/headers/rewrite";
    h.expect(false, "POST", rewrite, None, 202, "common#Message")
        .await;
    let running = h.fails("POST", rewrite, None, 409).await;
    assert_eq!(running["error"], "rewrite_running");
    h.ok(
        "POST",
        "/api/admin/access/recheck",
        Some(json!({})),
        "config#AccessReport",
    )
    .await;

    // History: pages and the three rollback previews.
    let page = h
        .get("/api/admin/history?limit=100", "history#HistoryPage")
        .await;
    let records = page["records"].as_array().unwrap().clone();
    assert!(!records.is_empty(), "the edits above wrote history");
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
    let actor = format!(
        "{}:{}",
        s(&newest["actor"]["kind"]),
        s(&newest["actor"]["id"])
    );
    let filtered = h
        .get(
            &format!(
                "/api/admin/history?limit=2&week={}&actor={actor}",
                week_key.replace('+', "%2B"),
            ),
            "history#HistoryPage",
        )
        .await;
    assert!(
        filtered["total"].as_u64().unwrap() > 0,
        "week filter by instant"
    );
    // A run's log: every record touching its row or RSVPs, newest first.
    let run_log = h
        .get(
            &format!("/api/admin/history?run={id}&limit=2"),
            "history#HistoryPage",
        )
        .await;
    assert!(
        run_log["total"].as_u64().unwrap() > 0,
        "the run has history"
    );
    assert!(
        run_log["records"].as_array().unwrap().iter().all(|r| {
            r["rows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["key"]["id"] == id || row["key"]["run_id"] == id)
        }),
        "{run_log}"
    );
    for bad in ["run=no-such-run", &format!("run={id}&actor=admin:token")] {
        h.fails("GET", &format!("/api/admin/history?{bad}"), None, 422)
            .await;
    }

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
    let plan = "history#RevertPlan";
    let revert = "/api/admin/history/revert";
    let strict = h
        .ok(
            "POST",
            revert,
            Some(json!({ "seqs": [first_move], "preview": true })),
            plan,
        )
        .await;
    assert_eq!(strict["outcome"], "conflicts", "{strict}");
    assert_eq!(strict["rows"], json!([]));
    assert_eq!(strict["reverts"], json!([first_move]));
    assert!(!strict["conflicts"].as_array().unwrap().is_empty());
    for (path, body) in [
        (revert, json!({ "seqs": [newest["seq"]], "preview": true })),
        (
            "/api/admin/history/restore-week",
            json!({ "week": week_key, "revision": 0, "preview": true }),
        ),
        (
            "/api/admin/history/revert-actor",
            json!({ "actor": actor, "since": "1970-01-01T00:00:00Z", "preview": true }),
        ),
        (revert, json!({ "seqs": [newest["seq"]] })),
    ] {
        h.ok("POST", path, Some(body), plan).await;
    }

    // Closing the portal closes the public schedule.
    h.config(json!({ "self_service": { "public_portal": false } }))
        .await;
    let status = h
        .expect(
            true,
            "GET",
            "/api/public/status",
            None,
            200,
            "public#PublicStatus",
        )
        .await;
    assert_eq!(status["portal"], "closed");
    h.expect(true, "GET", "/api/public/session", None, 503, "")
        .await;

    // Sign-in and sessions: methods, token login, sign-out, 401 while signed out, Discord.
    let (status, _, methods) = h
        .send_with(false, "GET", "/api/admin/auth/methods", None, NONE)
        .await;
    assert_eq!(status, StatusCode::OK);
    for key in ["discord", "tailscale", "token"] {
        assert!(methods[key].is_boolean(), "{key}: {methods}");
    }
    let session = h.get("/api/admin/session", "identity#Session").await;
    assert_eq!(session["method"], "discord");
    let token = "/api/admin/auth/token";
    let logout = "/api/admin/auth/logout";
    for (path, body, want) in [
        (token, json!({ "token": "wrong" }), (401, "unauthenticated")),
        (token, json!({ "tok": "x" }), (400, "invalid_body")),
        (logout, json!({}), (403, "csrf")),
    ] {
        h.refused_with("POST", path, body, NONE, want).await;
    }
    let csrf = h.csrf.clone();
    let (status, _, _) = h
        .send_with(false, "POST", logout, None, &[("x-kanade-csrf", &csrf)])
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.fails("GET", "/api/admin/week", None, 401).await;
    // Signed out, the sign-in strip still answers: time, boss names and tally only.
    let tonight = h
        .expect(
            false,
            "GET",
            "/api/admin/auth/tonight",
            None,
            200,
            "week#Tonight",
        )
        .await;
    assert_eq!(
        tonight,
        json!({ "run": { "time": "22:00", "bosses": ["Carling", "Radiant Malefic Star"], "tally": { "on": 4, "total": 7 } } })
    );
    let body = json!({ "token": crate::auth::MOCK_TOKEN });
    let (status, headers, session) = h.send_with(false, "POST", token, Some(body), NONE).await;
    assert_eq!(status, StatusCode::OK);
    h.check("token login", status, &session, "identity#Session");
    assert_eq!(session["method"], "token");
    let fresh = header_value(&headers, "x-kanade-csrf").to_owned();
    assert!(
        !fresh.is_empty() && fresh != csrf,
        "a sign-in issues a new CSRF token"
    );
    h.csrf = fresh;
    h.get("/api/admin/week", "week#Week").await;
    let me = h.get("/api/admin/me", "identity#Me").await;
    assert_eq!(me["member"], Value::Null, "a token session stays neutral");
    let start = |next: &str| format!("/api/admin/auth/discord/start?next={next}");
    let (status, headers, _) = h
        .send_with(false, "GET", &start("/inbox?tab=self_service"), None, NONE)
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let callback = header_value(&headers, "location").to_owned();
    let (status, _, page) = h.send_with(false, "GET", &callback, None, NONE).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.as_str()
            .is_some_and(|p| p.contains("url=/inbox?tab=self_service")),
        "{page}"
    );
    h.control(false, "/__mock/discord", json!({ "error": "forbidden" }))
        .await;
    let (status, headers, _) = h
        .send_with(false, "GET", &start("//evil.example/"), None, NONE)
        .await;
    assert_eq!(
        (status, header_value(&headers, "location")),
        (StatusCode::SEE_OTHER, "/?login_error=forbidden")
    );

    h.finish();
}

/// Walks the member auth routes (docs/notes/member-auth-contract.md §1–§3).
#[tokio::test]
async fn public_member_routes_match_the_contract() {
    let mut h = Harness::new();
    let session_routes = [
        "/api/public/session",
        "/api/public/sessions",
        "/api/public/week",
        "/api/public/events",
        "/api/public/me/allowance",
        "/api/public/bosses",
        "/api/public/bosses/events",
        "/api/public/bosses/Carling/knowledge",
        "/art/entry/Carling",
    ];

    // Open and signed out: status, then 401 for every session route.
    let (_, status) = h
        .public(
            "GET",
            "/api/public/status",
            NONE,
            200,
            "public#PublicStatus",
        )
        .await;
    assert_eq!(status["portal"], "open");
    for path in session_routes {
        h.public("GET", path, NONE, 401, "").await;
    }
    // Unmounted while open: the catch-all answers 404.
    h.public("GET", "/api/public/nope", NONE, 404, "").await;

    // Discord sign-in: start → (no Discord) callback → landing with the cookie.
    let (headers, _) = h
        .public(
            "GET",
            "/api/public/auth/discord/start?next=/account",
            NONE,
            303,
            "",
        )
        .await;
    let callback = header_value(&headers, "location").to_owned();
    assert!(
        callback.starts_with("/api/public/auth/discord/callback?"),
        "{callback}"
    );
    let ua = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";
    let (headers, page) = h
        .public("GET", &callback, &[("user-agent", ua)], 200, "")
        .await;
    assert!(
        page.as_str().is_some_and(|p| p.contains("url=/account")),
        "{page}"
    );
    assert!(
        header_value(&headers, "set-cookie").contains("HttpOnly; SameSite=Strict"),
        "{headers:?}"
    );
    let cookie = session_cookie(&headers);
    assert!(
        cookie.starts_with("kanade_pub=") && cookie.len() > 20,
        "{cookie}"
    );

    // Signed in: the session (token in the header only), the device list, the portrait.
    let auth: &[(&str, &str)] = &[("cookie", &cookie)];
    let (headers, session) = h
        .public(
            "GET",
            "/api/public/session",
            auth,
            200,
            "public#PublicSession",
        )
        .await;
    let token = header_value(&headers, "x-kanade-csrf").to_owned();
    assert!(!token.is_empty());
    assert!(
        !session.to_string().contains(&token),
        "the token is never in the body"
    );
    let (_, list) = h
        .public(
            "GET",
            "/api/public/sessions",
            auth,
            200,
            "public#PublicSessions",
        )
        .await;
    let rows = list["sessions"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 3);
    let mine = rows.iter().find(|r| r["current"] == true).unwrap();
    assert_eq!(mine["device"], "Chrome · macOS");
    let other = s(&rows.iter().find(|r| r["current"] == false).unwrap()["handle"]).to_owned();
    let avatar = "/api/public/session/avatar?v=1";
    h.image(true, avatar, &cookie, "private, no-cache").await;

    // Member reads (MemberWeek, MemberAllowance) against their schemas: own and
    // other runs, both weeks, then the art behind the session.
    for (path, next) in [
        ("/api/public/week", false),
        ("/api/public/week?week=this", false),
        ("/api/public/week?week=next", true),
    ] {
        let (_, week) = h.public("GET", path, auth, 200, "public#MemberWeek").await;
        let runs = week["runs"].as_array().cloned().unwrap_or_default();
        let mine = runs.iter().filter(|r| r["mine"] == true).count();
        h.verify(mine > 0 && (next || mine < runs.len()), || {
            format!("public week {path}: {mine} of {} runs mine", runs.len())
        });
    }
    h.public("GET", "/api/public/week?week=last", auth, 422, "")
        .await;
    h.public(
        "GET",
        "/api/public/me/allowance",
        auth,
        200,
        "public#MemberAllowance",
    )
    .await;
    // Boss guides: the admin list and event bosses as they are; knowledge
    // without `path` or any bullet's `detail` (the schema refuses either).
    h.public("GET", "/api/public/bosses", auth, 200, "bosses#BossRows")
        .await;
    let (_, events) = h
        .public(
            "GET",
            "/api/public/bosses/events",
            auth,
            200,
            "bosses#EventBosses",
        )
        .await;
    let kai = events
        .as_array()
        .is_some_and(|rows| rows.iter().any(|row| row["key"] == "Kai"));
    h.verify(kai, || format!("public event bosses: {events}"));
    for key in ["Carling", "MaleficStar", "Kai"] {
        let path = format!("/api/public/bosses/{key}/knowledge");
        let (_, knowledge) = h
            .public("GET", &path, auth, 200, "public#PublicKnowledge")
            .await;
        let leaks = knowledge.to_string().contains("\"detail\"") || knowledge.get("path").is_some();
        h.verify(!leaks, || {
            format!("public knowledge {key} leaks path or detail")
        });
    }
    h.public("GET", "/api/public/bosses/Nobody/knowledge", auth, 404, "")
        .await;
    // Behind the session, so per-user and revalidated on every use (as the server).
    h.image(
        true,
        "/art/entry/Carling",
        &cookie,
        "private, max-age=86400",
    )
    .await;

    // Writes need the session's token: sign out one device, then the refusals.
    let end_other = format!("/api/public/sessions/{other}");
    h.public("DELETE", &end_other, auth, 403, "").await;
    let signed = [
        ("cookie", cookie.as_str()),
        ("x-kanade-csrf", token.as_str()),
    ];
    h.public("DELETE", &end_other, &signed, 204, "").await;
    h.public("DELETE", &end_other, &signed, 404, "").await;
    let end_mine = format!("/api/public/sessions/{}", s(&mine["handle"]));
    let (_, refused) = h.public("DELETE", &end_mine, &signed, 409, "").await;
    assert_eq!(refused["error"], "current_session");

    // A rotation (client IP change): the request is served on the old id and
    // answers the new cookie and token; the old id is gone.
    h.public("POST", "/__mock/public/rotate", NONE, 204, "")
        .await;
    let (headers, _) = h
        .public(
            "GET",
            "/api/public/sessions",
            auth,
            200,
            "public#PublicSessions",
        )
        .await;
    let rotated = session_cookie(&headers);
    let fresh = header_value(&headers, "x-kanade-csrf").to_owned();
    assert!(rotated != cookie && !fresh.is_empty() && fresh != token);
    h.public("GET", "/api/public/session", auth, 401, "").await;

    // Sign out everywhere ends every session, this one too, and clears the cookie.
    let (headers, ended) = h
        .public(
            "POST",
            "/api/public/sessions/end-all",
            &[("cookie", &rotated), ("x-kanade-csrf", &fresh)],
            200,
            "identity#SessionsEnded",
        )
        .await;
    assert_eq!(ended["ended"], 2);
    assert_eq!(session_cookie(&headers), "kanade_pub=");
    h.public(
        "GET",
        "/api/public/sessions",
        &[("cookie", &rotated)],
        401,
        "",
    )
    .await;

    // Sign out (the shortcut signs in without Discord): a live session needs its token.
    let (cookie, token) = h.sign_in_public().await;
    let logout = "/api/public/auth/logout";
    h.public("POST", logout, &[("cookie", &cookie)], 403, "")
        .await;
    let (headers, _) = h
        .public(
            "POST",
            logout,
            &[("cookie", &cookie), ("x-kanade-csrf", &token)],
            204,
            "",
        )
        .await;
    assert_eq!(session_cookie(&headers), "kanade_pub=");
    h.public(
        "GET",
        "/api/public/session",
        &[("cookie", &cookie)],
        401,
        "",
    )
    .await;

    // A member who is not eligible: back to the app with the code, no session cookie.
    let (status, _, _) = h
        .send_with(
            true,
            "POST",
            "/__mock/public/discord",
            Some(json!({ "error": "not_eligible" })),
            NONE,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (headers, _) = h
        .public(
            "GET",
            "/api/public/auth/discord/start?next=/",
            NONE,
            303,
            "",
        )
        .await;
    assert_eq!(
        header_value(&headers, "location"),
        "/?login_error=not_eligible"
    );
    assert!(headers.get("set-cookie").is_none());

    // Closed: status says so; session routes and unmounted paths answer
    // `closed`; sign-in goes back with `login_error=closed`; sign-out still works.
    h.config(json!({ "self_service": { "public_portal": false } }))
        .await;
    let (_, status) = h
        .public(
            "GET",
            "/api/public/status",
            NONE,
            200,
            "public#PublicStatus",
        )
        .await;
    assert_eq!(status["portal"], "closed");
    for path in session_routes {
        let (_, refused) = h.public("GET", path, NONE, 503, "").await;
        assert_eq!(refused["error"], "closed", "{path}");
    }
    for path in [
        "/api/public/auth/discord/start?next=/",
        "/api/public/auth/discord/callback?next=/",
    ] {
        let (headers, _) = h.public("GET", path, NONE, 303, "").await;
        assert_eq!(header_value(&headers, "location"), "/?login_error=closed");
    }
    h.public("POST", logout, NONE, 204, "").await;
    // The admin routes stay absent from this origin.
    h.public("GET", "/api/admin/week", NONE, 404, "").await;

    h.finish();
}

#[tokio::test]
async fn public_member_ownership_matches_the_contract() {
    let mut h = Harness::new();
    let timings = "/api/public/timings";
    let ask = "/api/public/timings/f-jupiter/owner-requests";
    let hand_off = "/api/public/timings/f-carling/owner";
    let writes = [
        hand_off,
        ask,
        "/api/public/owner-requests/own-kalos/accept",
        "/api/public/owner-requests/own-kalos/decline",
        "/api/public/owner-requests/own-kalos/withdraw",
    ];
    h.public("GET", timings, NONE, 401, "").await;
    for path in writes {
        h.public("POST", path, &[("idempotency-key", "k-0")], 401, "")
            .await;
    }

    let (cookie, token) = h.sign_in_public().await;
    let auth = [("cookie", cookie.as_str())];

    // The timings Asahi is on: her own Carling shows Mika's request; Ren's
    // Kalos does not show Tsubame's.
    let (_, listed) = h
        .public("GET", timings, &auth, 200, "public#MemberTimings")
        .await;
    let find = |listed: &Value, id: &str| {
        listed["timings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} missing: {listed}"))
    };
    let carling = find(&listed, "f-carling");
    assert_eq!(carling["you_own"], true);
    assert_eq!(carling["requests"][0]["id"], "own-carling");
    assert_eq!(find(&listed, "f-kalos")["requests"], json!([]));
    // Unmounted methods on the new paths.
    h.public("POST", timings, &auth, 404, "").await;
    h.public("GET", hand_off, &auth, 404, "").await;

    // Writes need the token and a well-formed key.
    let k = |key: &'static str| signed(&cookie, &token, key);
    let ask_1 = k("ask-1");
    let no_token = [("cookie", cookie.as_str()), ("idempotency-key", "ask-1")];
    h.public("POST", ask, &no_token, 403, "").await;
    let no_key = [
        ("cookie", cookie.as_str()),
        ("x-kanade-csrf", token.as_str()),
    ];
    for headers in [&no_key[..], &k("a b")] {
        let (_, refused) = h.public("POST", ask, headers, 400, "").await;
        assert_eq!(refused["error"], "invalid_idempotency_key");
    }

    // Ask: 201, the same key 200 with the same request, another key 409.
    let request = "public#MemberOwnerRequest";
    let (_, asked) = h.public("POST", ask, &ask_1, 201, request).await;
    assert_eq!(
        (&asked["status"], &asked["mine"]),
        (&json!("open"), &json!(true))
    );
    let (_, again) = h.public("POST", ask, &ask_1, 200, request).await;
    assert_eq!(again, asked);
    let ask_on = |timing: &str| format!("/api/public/timings/{timing}/owner-requests");
    let decide = |id: &str, verdict: &str| format!("/api/public/owner-requests/{id}/{verdict}");
    for (path, key, want, code) in [
        (ask.to_owned(), "ask-2", 409, "already_asked"),
        (ask_on("f-kalos"), "ask-1", 422, "idempotency_mismatch"),
        (ask_on("f-baldrix"), "ask-3", 409, "already_owner"),
        (ask_on("f-limbo"), "ask-4", 409, "not_on_party"),
        (ask_on("f-none"), "ask-5", 404, "not_found"),
        // Someone else's ask on a timing the caller doesn't own: the same 404
        // as an unknown id (no existence oracle).
        (decide("own-kalos", "accept"), "acc-1", 404, "not_found"),
        (
            decide("own-carling", "withdraw"),
            "wd-1",
            403,
            "not_requester",
        ),
        (decide("nope", "decline"), "dec-1", 404, "not_found"),
    ] {
        let (_, refused) = h.public("POST", &path, &k(key), want, "").await;
        assert_eq!(refused["error"], code, "{path}");
    }

    // Hand-off: a snowflake `to` and nothing else.
    for body in [
        json!({ "to": "1002" }),
        json!({ "to": 100_000_000_000_001_002_u64 }),
        json!({ "to": "100000000000001002", "staff": true }),
        json!({}),
    ] {
        let (_, refused) = h
            .public_with("POST", hand_off, Some(body.clone()), &k("h-0"), 422, "")
            .await;
        assert_eq!(refused["error"], "invalid_body", "{body}");
    }
    let to = |id: &str| Some(json!({ "to": id }));
    let timing = "public#MemberTiming";
    let (_, handed) = h
        .public_with(
            "POST",
            hand_off,
            to("100000000000001002"),
            &k("h-1"),
            200,
            timing,
        )
        .await;
    assert_eq!(handed["owner"]["id"], "100000000000001002");
    assert_eq!(
        (
            &handed["owner_pinned"],
            &handed["you_own"],
            &handed["requests"]
        ),
        (&json!(true), &json!(false), &json!([])),
        "Mika's request went with it"
    );
    h.public_with(
        "POST",
        hand_off,
        to("100000000000001002"),
        &k("h-1"),
        200,
        timing,
    )
    .await;
    let (_, refused) = h
        .public_with(
            "POST",
            hand_off,
            to("100000000000001003"),
            &k("h-2"),
            403,
            "",
        )
        .await;
    assert_eq!(refused["error"], "not_owner");

    // Withdraw her own ask; a second time it is closed.
    let withdraw = decide(s(&asked["id"]), "withdraw");
    let (_, withdrawn) = h.public("POST", &withdraw, &k("wd-2"), 200, request).await;
    assert_eq!(withdrawn["status"], "withdrawn");
    let (_, refused) = h.public("POST", &withdraw, &k("wd-3"), 409, "").await;
    assert_eq!(refused["error"], "request_closed");

    // The Inbox shares the requests: Mika's is gone, Tsubame's stays.
    let open = h
        .get("/api/admin/inbox/ownership", "inbox#OwnershipRequests")
        .await;
    assert_eq!(open[0]["id"], "own-kalos");
    assert_eq!(open.as_array().unwrap().len(), 1);

    // Closed: every route answers `closed`.
    h.config(json!({ "self_service": { "public_portal": false } }))
        .await;
    let (_, refused) = h.public("GET", timings, &auth, 503, "").await;
    assert_eq!(refused["error"], "closed");
    for path in writes {
        let (_, refused) = h.public("POST", path, &k("k-9"), 503, "").await;
        assert_eq!(refused["error"], "closed", "{path}");
    }

    h.finish();
}

/// The member writes' routes (member-writes-contract): the session, the
/// token, a required key with replay, a fresh sign-in on all but a
/// withdrawal, and the refusal codes. The bodies' schemas belong to the API
/// lane's `public.json`; here only status and error codes are walked.
#[tokio::test]
async fn public_member_writes_follow_the_contract() {
    let mut h = Harness::new();
    let answer = "/api/public/runs/r-carling/answer";
    let moved = "/api/public/runs/r-carling/move";
    let link = "/api/public/runs/r-carling";
    let submit = "/api/public/requests";
    let withdraw = "/api/public/requests/req-kalos-weekly/withdraw";

    // Signed out: 401 everywhere.
    for (method, path) in [
        ("GET", link),
        ("PUT", answer),
        ("POST", moved),
        ("GET", "/api/public/requests/mine"),
        ("POST", submit),
        ("POST", withdraw),
    ] {
        let (status, _, _) = h.send_with(true, method, path, None, NONE).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    let (cookie, token) = h.sign_in_public().await;
    let h = &h;
    // Status, error code and body of a member call with these headers.
    let call = async |method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]| {
        let (status, _, value) = h.send_with(true, method, path, body, headers).await;
        let code = value["error"].as_str().unwrap_or_default().to_owned();
        (status.as_u16(), code, value)
    };
    let read = [("cookie", cookie.as_str())];
    let with = |key: &'static str| signed(&cookie, &token, key);

    // The link view needs only the session.
    let (status, _, view) = call("GET", link, None, &read).await;
    assert_eq!((status, view["week"].as_str()), (200, Some("current")));
    let (status, code, _) = call("GET", "/api/public/runs/nope", None, &read).await;
    assert_eq!((status, code.as_str()), (404, "not_found"));

    // Writes: no token 403, no key 400, a bad body 422.
    let body = json!({ "answer": "maybe", "version": 1 });
    for (headers, want) in [
        (
            &[("cookie", cookie.as_str()), ("idempotency-key", "k")][..],
            (403, "csrf"),
        ),
        (
            &[
                ("cookie", cookie.as_str()),
                ("x-kanade-csrf", token.as_str()),
            ][..],
            (400, "invalid_idempotency_key"),
        ),
    ] {
        let (status, code, _) = call("PUT", answer, Some(body.clone()), headers).await;
        assert_eq!((status, code.as_str()), want);
    }
    let maybe = Some(json!({ "answer": "maybe" }));
    let (status, code, _) = call("PUT", answer, maybe, &with("bad")).await;
    assert_eq!((status, code.as_str()), (422, "invalid_body"));

    // The week's version, then an answer, its replay and a mismatched reuse of the key.
    let (_, _, week) = call("GET", "/api/public/week", None, &read).await;
    let v = week["version"].as_u64().unwrap();
    let body = json!({ "answer": "maybe", "version": v });
    let (status, _, first) = call("PUT", answer, Some(body.clone()), &with("a-1")).await;
    assert_eq!(status, 200);
    let (status, _, again) = call("PUT", answer, Some(body), &with("a-1")).await;
    assert_eq!((status, &again), (200, &first));
    let no = Some(json!({ "answer": "no", "version": v }));
    let (status, code, _) = call("PUT", answer, no, &with("a-1")).await;
    assert_eq!((status, code.as_str()), (422, "idempotency_mismatch"));
    let v = first["version"].as_u64().unwrap();
    let to = |time: &str| Some(json!({ "day": 6, "time": time, "version": v }));
    let started = "/api/public/runs/r-kalos/move";
    let (status, code, _) = call("POST", started, to("21:00"), &with("m-1")).await;
    assert_eq!((status, code.as_str()), (409, "run_started"));
    let (status, _, moved_run) = call("POST", moved, to("21:30"), &with("m-2")).await;
    assert_eq!(
        (status, moved_run["previous"]["time"].as_str()),
        (200, Some("22:00"))
    );

    // Requests: the list, a submit (201, then 200 on retry), the open limit with its `limit`.
    let (status, _, mine) = call("GET", "/api/public/requests/mine", None, &read).await;
    assert_eq!((status, mine["open"].as_u64()), (200, Some(1)));
    let leave = |run: &str| Some(json!({ "kind": "leave", "run_id": run }));
    let (status, _, sent) = call("POST", submit, leave("r-carling"), &with("r-1")).await;
    assert_eq!((status, sent["state"].as_str()), (201, Some("waiting")));
    let (status, _, retried) = call("POST", submit, leave("r-carling"), &with("r-1")).await;
    assert_eq!((status, &retried["id"]), (200, &sent["id"]));
    let extra = Some(json!({ "kind": "leave", "run_id": "r-carling", "extra": 1 }));
    let (status, code, _) = call("POST", submit, extra, &with("r-2")).await;
    assert_eq!((status, code.as_str()), (422, "invalid_body"));
    call("POST", submit, leave("n-carling"), &with("r-3")).await;
    let (status, code, limited) = call("POST", submit, leave("n-kalos"), &with("r-4")).await;
    assert_eq!(
        (status, code.as_str(), limited["limit"].as_str()),
        (429, "request_limit", Some("open"))
    );

    // A sign-in older than the fresh window: every write but a withdrawal asks for a new one.
    h.send_with(true, "POST", "/__mock/public/unfresh", None, NONE)
        .await;
    for (method, path, body) in [
        (
            "PUT",
            answer,
            Some(json!({ "answer": "yes", "version": v })),
        ),
        ("POST", moved, to("22:00")),
        ("POST", submit, leave("r-jupiter")),
    ] {
        let (status, code, _) = call(method, path, body, &with("fresh-1")).await;
        assert_eq!((status, code.as_str()), (401, "reauth_required"), "{path}");
    }
    let (status, _, withdrawn) = call("POST", withdraw, Some(json!({})), &with("w-1")).await;
    assert_eq!(
        (status, withdrawn["state"].as_str()),
        (200, Some("withdrawn"))
    );
    let (status, code, _) = call("POST", withdraw, Some(json!({})), &with("w-2")).await;
    assert_eq!((status, code.as_str()), (409, "request_closed"));

    // Closed: every route answers `closed`.
    let body = json!({ "self_service": { "public_portal": false } });
    h.send(false, "PATCH", "/api/admin/config", Some(body))
        .await;
    let (status, code, _) = call("GET", link, None, &read).await;
    assert_eq!((status, code.as_str()), (503, "closed"));
}

impl Harness {
    /// An event stream as `EventSource` sends it: the SSE text after the
    /// shared checks (`200`, `text/event-stream`, never cached).
    async fn stream(&self, public: bool, path: &str, cookie: &str, last: Option<&str>) -> String {
        let mut req = Request::get(path);
        if !cookie.is_empty() {
            req = req.header(header::COOKIE, cookie);
        }
        if let Some(last) = last {
            req = req.header("last-event-id", last);
        }
        let res = self
            .router(public)
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
        assert_eq!(
            res.headers()[header::CACHE_CONTROL],
            "no-store, no-transform"
        );
        if public {
            // The stream never rotates the session (nor touches it).
            assert!(res.headers().get(header::SET_COOKIE).is_none());
        }
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// `GET /api/admin/events`: the SSE text.
    async fn events(&self, last: Option<u64>) -> String {
        let last = last.map(|n| n.to_string());
        self.stream(false, "/api/admin/events", "", last.as_deref())
            .await
    }

    /// Each `data:` line's JSON (hints carry `topic` and `seq` only).
    async fn hints(&self, last: u64) -> Vec<Value> {
        data_lines(&self.events(Some(last)).await)
            .inspect(|hint| {
                let keys: Vec<&String> = hint.as_object().unwrap().keys().collect();
                assert_eq!(keys, ["seq", "topic"], "no data in a hint: {hint}");
            })
            .collect()
    }
}

fn data_lines(text: &str) -> impl Iterator<Item = Value> {
    text.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str::<Value>(data).unwrap())
}

fn find<'a>(rows: &'a Value, key: &str, value: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|row| row[key] == value)
        .unwrap_or_else(|| panic!("no {key} {value}"))
}

#[tokio::test]
async fn events_hint_own_writes_and_arrivals_and_the_reads_change() {
    let mut h = Harness::new();
    let opening = h.events(None).await;
    assert!(
        opening.contains("event: ready\ndata: {\"boot\":\"mock-") && opening.contains("\"seq\":0}"),
        "{opening}"
    );

    let week = h.get("/api/admin/week", "week#Week").await;
    let version = week["version"].as_u64().unwrap();
    h.ok(
        "POST",
        "/api/admin/runs/r-limbo/move",
        Some(json!({ "day": 2, "time": "23:30", "version": version })),
        "week#MoveResult",
    )
    .await;
    assert_eq!(h.hints(0).await, [json!({ "seq": 1, "topic": "schedule" })]);

    for (kind, topic) in [
        ("reaction", "schedule"),
        ("move", "schedule"),
        ("run", "schedule"),
        ("proposal", "inbox"),
        ("chat", "chat"),
        ("extraction", "extraction"),
        ("member", "members"),
    ] {
        let status = h
            .control(false, "/__mock/arrive", json!({ "kind": kind }))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{kind}");
        let last = h.hints(0).await.last().cloned().unwrap();
        assert_eq!(last["topic"], topic, "{kind}");
    }

    let week = h.get("/api/admin/week", "week#Week").await;
    let kalos = find(&week["runs"], "id", "r-kalos");
    let ren = find(&kalos["participants"], "id", "1005");
    assert_eq!(ren["answer"], "yes", "the reaction landed");
    let members = h.get("/api/admin/members", "members#MemberRows").await;
    let mika = find(&members, "id", "1003");
    assert!(
        mika["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "mikan"),
        "the roster sync landed: {mika}"
    );
    let inbox = h.get("/api/admin/inbox", "inbox#Proposals").await;
    find(&inbox, "id", "p-arrived");
    let chat = h.get("/api/admin/chat", "chat#Chat").await;
    assert_eq!(chat["rows"][0]["id"], "c-arrived", "newest first");
    h.get("/api/admin/chat/c-arrived", "chat#ChatTurn").await;
    let calls = h
        .get("/api/admin/extractions", "extractions#Extractions")
        .await;
    assert_eq!(calls["rows"][0]["id"], "x-arrived", "newest first");
    h.get("/api/admin/extractions/x-arrived", "extractions#Extraction")
        .await;
    h.finish();
}

#[tokio::test]
async fn the_member_stream_sends_topics_only_from_its_control() {
    let mut h = Harness::new();
    let (cookie, _) = h.sign_in_public().await;
    let events = "/api/public/events";
    let opening = h.stream(true, events, &cookie, None).await;
    assert!(
        opening.contains("event: ready\ndata: {\"boot\":\"mock-") && !opening.contains("seq"),
        "{opening}"
    );
    for topic in ["schedule", "mine", "allowance"] {
        let status = h
            .control(true, "/__mock/public/hint", json!({ "topic": topic }))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{topic}");
    }
    let hints = h.stream(true, events, &cookie, Some("0")).await;
    assert_eq!(
        data_lines(&hints).collect::<Vec<_>>(),
        [
            json!({ "topic": "schedule" }),
            json!({ "topic": "mine" }),
            json!({ "topic": "allowance" })
        ]
    );
    let status = h
        .control(true, "/__mock/public/hint", json!({ "topic": "chat" }))
        .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "member topics only"
    );
    // The admin stream heard none of it.
    let admin = h.events(None).await;
    assert!(admin.contains("\"seq\":0}"), "{admin}");

    // A pending rotation waits for the next ordinary request, not the stream.
    h.public("POST", "/__mock/public/rotate", NONE, 204, "")
        .await;
    h.stream(true, events, &cookie, None).await;
    let (headers, _) = h
        .public(
            "GET",
            "/api/public/sessions",
            &[("cookie", &cookie)],
            200,
            "public#PublicSessions",
        )
        .await;
    assert_ne!(session_cookie(&headers), cookie, "rotated here");
}

#[tokio::test]
async fn admin_reads_revalidate_with_etags() {
    let h = Harness::new();
    for path in [
        "/api/admin/week",
        "/api/admin/summary",
        "/api/admin/inbox",
        "/api/admin/chat",
    ] {
        let (status, headers, body) = h.send_with(false, "GET", path, None, NONE).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        let tag = headers[header::ETAG].to_str().unwrap().to_owned();
        let (status, again, empty) = h
            .send_with(false, "GET", path, None, &[("if-none-match", &tag)])
            .await;
        assert_eq!(status, StatusCode::NOT_MODIFIED, "{path}");
        assert_eq!(empty, Value::Null, "{path}: no body");
        assert_eq!(again[header::ETAG], tag.as_str());
        assert_eq!(again[header::CACHE_CONTROL], headers[header::CACHE_CONTROL]);
        let (status, _, fresh) = h
            .send_with(false, "GET", path, None, &[("if-none-match", "\"other\"")])
            .await;
        assert_eq!((status, fresh), (StatusCode::OK, body), "{path}");
    }
}

#[tokio::test]
async fn inbox_ownership_requests_match_the_contract() {
    let mut h = Harness::new();
    let list = "/api/admin/inbox/ownership";
    let open = h.get(list, "inbox#OwnershipRequests").await;
    let ids: Vec<&str> = open
        .as_array()
        .unwrap()
        .iter()
        .map(|r| s(&r["id"]))
        .collect();
    assert_eq!(ids, ["own-carling", "own-kalos"], "oldest first");
    let summary = h.get("/api/admin/summary", "week#Summary").await;
    let inbox = h.get("/api/admin/inbox", "inbox#Proposals").await;
    assert_eq!(
        summary["inbox"].as_u64(),
        Some(inbox.as_array().unwrap().len() as u64 + 2),
        "the summary counts open ownership requests"
    );

    // CSRF is required; an unknown id is 404.
    let accept = "/api/admin/inbox/ownership/own-kalos/accept";
    h.refused_with("POST", accept, json!({}), NONE, (403, "csrf"))
        .await;
    for action in ["accept", "decline"] {
        let path = format!("/api/admin/inbox/ownership/nope/{action}");
        h.refused("POST", &path, json!({}), (404, "not_found"))
            .await;
    }

    // Any session decides, Discord or not: accept pins the requester.
    h.sign_in_admin("token").await;
    let first = h
        .ok("POST", accept, Some(json!({})), "common#Message")
        .await;
    let again = h
        .ok("POST", accept, Some(json!({})), "common#Message")
        .await;
    assert_eq!(first, again, "a repeated accept answers the first result");
    let fixed = h.get("/api/admin/fixed", "fixed#FixedRows").await;
    let kalos = find(&fixed, "id", "f-kalos");
    assert_eq!(
        (&kalos["owner_id"], &kalos["owner_pinned"]),
        (&json!("1005"), &json!(true))
    );
    h.ok(
        "POST",
        "/api/admin/inbox/ownership/own-carling/decline",
        Some(json!({})),
        "common#Message",
    )
    .await;
    h.refused(
        "POST",
        "/api/admin/inbox/ownership/own-carling/accept",
        json!({}),
        (409, "conflicts"),
    )
    .await;
    let left = h.get(list, "inbox#OwnershipRequests").await;
    assert_eq!(left, json!([]));

    // Signed out, the routes answer 401.
    let (status, _) = h.send(false, "POST", "/api/admin/auth/logout", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    h.fails("GET", list, None, 401).await;
    h.refused("POST", accept, json!({}), (401, "unauthenticated"))
        .await;
    h.finish();
}

#[tokio::test]
async fn estimates_match_the_contract() {
    let mut h = Harness::new();
    let job = "character_data#EstimateJob";
    let started = "character_data#EstimateStarted";
    let estimates = "/api/admin/estimates";
    let job_path = |job: &Value| format!("{estimates}/{}", s(&job["job"]["id"]));
    // The seeded MikaNL lookup is found again.
    let seeded = h
        .get(
            &format!("{estimates}?character_ids=ch-s2"),
            "character_data#EstimateStatus",
        )
        .await;
    assert_eq!(seeded["job"]["state"], "done");
    // A run steps one character per read, then the total.
    let body = json!({ "run_id": "r-carling" });
    let first = h
        .expect(false, "POST", estimates, Some(body), 202, started)
        .await;
    let first = job_path(&first);
    let s3 = json!({ "character_ids": ["ch-s3"] });
    h.refused_with("POST", estimates, s3.clone(), NONE, (403, "csrf"))
        .await;
    // A second lookup waits in line behind the running one.
    let queued = h
        .expect(false, "POST", estimates, Some(s3), 202, started)
        .await;
    assert_eq!(queued["job"]["state"], "queued");
    assert_eq!(queued["job"]["position"], 1);
    let mut last = Value::Null;
    for _ in 0..8 {
        last = h.get(&first, job).await;
        if last["state"] != "running" {
            break;
        }
    }
    assert_eq!(last["state"], "done");
    let next = h.get(&job_path(&queued), job).await;
    assert_eq!(next["state"], "done", "it ran once the line moved");
    // The queued and assumed scenarios.
    for scenario in ["queued", "assumed"] {
        h.control(false, "/__mock/estimates", json!({ "scenario": scenario }))
            .await;
        let body = json!({ "run_id": "r-carling", "refresh": true });
        let begun = h
            .expect(false, "POST", estimates, Some(body), 202, started)
            .await;
        let path = job_path(&begun);
        let mut read = begun["job"].clone();
        while read["state"] != "done" {
            read = h.get(&path, job).await;
        }
        h.get(
            "/api/admin/runs/r-carling/readiness",
            "character_data#RunReadiness",
        )
        .await;
    }
    // Every error state, the hold (running) and off.
    for scenario in ["busy", "critical_rate", "manual_check", "failed", "timeout"] {
        h.control(false, "/__mock/estimates", json!({ "scenario": scenario }))
            .await;
        let body = json!({ "character_ids": ["ch-s3"], "refresh": true });
        let begun = h
            .expect(false, "POST", estimates, Some(body), 202, started)
            .await;
        let failed = h.get(&job_path(&begun), job).await;
        assert_eq!(failed["state"], "failed", "{scenario}");
    }
    h.refused(
        "POST",
        estimates,
        json!({ "run_id": "r-carling", "preset": "00110" }),
        (400, "invalid_body"),
    )
    .await;
    h.control(false, "/__mock/estimates", json!({ "scenario": "off" }))
        .await;
    h.fails("GET", &first, None, 404).await;
    h.finish();
}

#[tokio::test]
async fn character_links_and_health_match_the_contract() {
    let mut h = Harness::new();
    let link = "character_data#CharacterLink";
    let config = "config#ConfigView";
    let link_path = |id: &str| format!("/api/admin/characters/{id}/link");
    // Every seeded state: linked, retired-linked, stale, pending, not linked, expired.
    let mut states = Vec::new();
    for id in ["ch-s1", "ch-s3", "ch-s4", "ch-s5", "ch-s7", "ch-s10"] {
        let view = h.get(&link_path(id), link).await;
        states.push(view["state"].as_str().unwrap_or_default().to_owned());
    }
    assert_eq!(
        states,
        [
            "linked",
            "linked",
            "stale",
            "pending",
            "not_linked",
            "expired"
        ]
    );
    // An admin request makes a pending link; Unlink withdraws it.
    let body = json!({ "version": 0 });
    let pending = h
        .expect(false, "POST", &link_path("ch-s7"), Some(body), 201, link)
        .await;
    assert_eq!(pending["state"], "pending");
    h.ok("DELETE", &link_path("ch-s7"), None, "common#Message")
        .await;
    // An admin Refresh is always allowed: expired and fresh alike.
    for id in ["ch-s10", "ch-s3"] {
        let path = format!("/api/admin/characters/{id}/refresh");
        let refreshed = h.ok("POST", &path, None, link).await;
        assert_eq!(refreshed["state"], "linked", "{id}");
    }
    // Held calls refuse it.
    let health = |health: &str| json!({ "health": health });
    h.control(false, "/__mock/character-data", health("refused"))
        .await;
    h.refused(
        "POST",
        "/api/admin/characters/ch-s3/refresh",
        json!({}),
        (503, "nexon_unavailable"),
    )
    .await;
    // Every health the boards draw validates; setup (last) has no key.
    for state in ["ok", "alert", "refused", "setup"] {
        h.control(false, "/__mock/character-data", health(state))
            .await;
        let view = h.get("/api/admin/config", config).await;
        assert!(view["character_data"].is_object(), "{state}");
    }
    let none = h.get(&link_path("ch-s7"), link).await;
    assert_eq!(none["state"], "not_connected");
    // Off: no health block, and the link routes answer the generic 404.
    h.control(false, "/__mock/modules", json!({ "character_data": false }))
        .await;
    let view = h.get("/api/admin/config", config).await;
    assert!(view.get("character_data").is_none());
    h.fails("GET", &link_path("ch-s3"), None, 404).await;
    h.finish();
}

#[tokio::test]
async fn character_lookup_matches_the_contract() {
    let mut h = Harness::new();
    let path = "/api/admin/characters/lookup";
    let status = h.get(path, "character_data#LookupStatus").await;
    assert_eq!(status["connected"], true);
    let found = h
        .ok(
            "POST",
            path,
            Some(json!({ "ign": "Wisteria" })),
            "character_data#CharacterLookup",
        )
        .await;
    assert_eq!(found["name"], "Wisteria");
    assert_eq!(found["attribution"], "Data based on NEXON Open API");
    for (body, want) in [
        (json!({ "ign": "NopeName" }), (404, "not_found")),
        (json!({ "ign": "BusyName" }), (503, "nexon_unavailable")),
        (json!({ "ign": "OfflineAb" }), (409, "not_connected")),
        (json!({ "ign": "WaitName" }), (429, "lookup_cooldown")),
        (
            json!({ "ign": "Wisteria", "world": "gms" }),
            (422, "region"),
        ),
        (json!({ "ign": "ab" }), (422, "invalid")),
    ] {
        h.refused("POST", path, body, want).await;
    }
    // A lookup keeps nothing: Mika's roster is as seeded.
    let mika = h
        .get(
            "/api/admin/members/1003/characters",
            "characters#MemberCharacters",
        )
        .await;
    assert_eq!(mika["characters"].as_array().map(Vec::len), Some(4));

    // Another region keeps its manual values and asks no link.
    let chars = "/api/admin/members/1013/characters";
    let created = "characters#CharacterCreated";
    let body = json!({ "ign": "Primrose", "world": "gms", "class": "Hero", "level": 250 });
    let other = h
        .expect(false, "POST", chars, Some(body), 201, created)
        .await;
    assert_eq!(
        (&other["level"], &other["link"]),
        (&json!(250), &Value::Null)
    );
    let asked = h
        .get(
            &format!("/api/admin/characters/{}/link", s(&other["id"])),
            "character_data#CharacterLink",
        )
        .await;
    assert_eq!(asked["state"], "not_linked");

    // Without a key: not connected, and a create asks nothing.
    h.control(
        false,
        "/__mock/character-data",
        json!({ "health": "setup" }),
    )
    .await;
    let status = h.get(path, "character_data#LookupStatus").await;
    assert_eq!(status["connected"], false);
    let body = json!({ "ign": "Heather9" });
    let plain = h
        .expect(false, "POST", chars, Some(body), 201, created)
        .await;
    assert_eq!(plain["link"], Value::Null);
    // Off: the generic 404.
    h.control(false, "/__mock/modules", json!({ "character_data": false }))
        .await;
    h.fails("GET", path, None, 404).await;
    h.finish();
}

/// The member's own characters on the public origin: every route against
/// `public.json` (and the reused `character_data.json`/`characters.json`
/// shapes), over the boards' roster (`POST /__mock/public/characters`).
#[tokio::test]
async fn public_member_characters_match_the_contract() {
    let mut h = Harness::new();
    let mine = "/api/public/me/characters";
    h.public("GET", mine, NONE, 401, "").await;
    let (cookie, token) = h.sign_in_public().await;
    let read = [("cookie", cookie.as_str())];
    let marked = [
        ("cookie", cookie.as_str()),
        ("x-kanade-csrf", token.as_str()),
    ];
    let key = |key: &'static str| signed(&cookie, &token, key);
    h.send_with(
        true,
        "POST",
        "/__mock/public/characters",
        Some(json!({ "roster": "board" })),
        NONE,
    )
    .await;

    // The roster, the request waiting first, the notice and Cancel.
    let (_, list) = h
        .public("GET", mine, &read, 200, "public#MyCharacters")
        .await;
    assert_eq!(list["characters"].as_array().unwrap().len(), 5);
    assert_eq!(list["pending"][0]["character_id"], "ch-b2");
    assert_eq!(list["pending"][0]["requested_by_name"], "Ren");
    let states: Vec<&str> = ["ch-b1", "ch-b2", "ch-b3", "ch-b4", "ch-b5"]
        .iter()
        .map(|id| list["links"][id]["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        ["linked", "pending", "stale", "expired", "not_linked"]
    );
    let b2 = "/api/public/me/characters/ch-b2/link";
    let (_, notice) = h
        .public(
            "GET",
            &format!("{b2}/notice"),
            &read,
            200,
            "public#LinkNotice",
        )
        .await;
    assert_eq!(
        notice["lead"],
        "Ren asked to link AsahiDK (MapleSEA). Nothing is fetched until you press Link."
    );
    let (_, kept) = h
        .public(
            "POST",
            &format!("{b2}/cancel"),
            &marked,
            200,
            "public#LinkCancelled",
        )
        .await;
    assert_eq!(kept["request_kept"], true);
    let (_, data) = h
        .public("GET", "/api/public/me/data", &read, 200, "public#MyData")
        .await;
    assert_eq!(data["entries"][0]["state"], "pending");

    // Link (consent on the portal), Unlink, Unlink all.
    let notice = |n: u64| Some(json!({ "notice": n }));
    let (_, outdated) = h
        .public_with("POST", b2, notice(2), &key("l-0"), 409, "")
        .await;
    assert_eq!(outdated["error"], "notice_changed");
    let (_, linked) = h
        .public_with(
            "POST",
            b2,
            notice(1),
            &key("l-1"),
            200,
            "public#LinkOutcome",
        )
        .await;
    assert_eq!(
        (&linked["state"], &linked["link"]["consent_surface"]),
        (&json!("linked"), &json!("portal"))
    );
    let (_, gone) = h
        .public(
            "DELETE",
            "/api/public/me/characters/ch-b1/link",
            &key("u-1"),
            200,
            "public#Unlinked",
        )
        .await;
    assert_eq!(gone, json!({"unlinked": "AsahiKanna", "cards": 1}));
    let (_, all) = h
        .public(
            "DELETE",
            "/api/public/me/characters/links",
            &key("u-2"),
            200,
            "public#UnlinkedAll",
        )
        .await;
    assert_eq!(all["unlinked"].as_array().unwrap().len(), 3);

    // Add, Edit, the lookup preview.
    let add = Some(json!({ "ign": "AsahiLumi" }));
    let created = "public#MyCharacterCreated";
    let (_, first) = h
        .public_with("POST", mine, add.clone(), &key("a-1"), 201, created)
        .await;
    assert_eq!(first["linkable"], true);
    let (_, again) = h
        .public_with("POST", mine, add, &key("a-1"), 201, created)
        .await;
    assert_eq!(again, first);
    let manual = Some(json!({ "ign": "AsahiGms", "world": "gms" }));
    let (_, manual) = h
        .public_with("POST", mine, manual, &key("a-2"), 422, "")
        .await;
    assert_eq!(manual["error"], "class_level_required");
    h.public_with(
        "PATCH",
        "/api/public/me/characters/ch-b3",
        Some(json!({ "version": 1, "class": "Pathfinder" })),
        &key("e-1"),
        200,
        "characters#CharacterRow",
    )
    .await;
    h.public_with(
        "POST",
        "/api/public/me/characters/lookup",
        Some(json!({ "ign": "AsahiLumi" })),
        &marked,
        200,
        "character_data#CharacterLookup",
    )
    .await;

    // A lookup of her own character, then the 30-minute limit.
    let estimates = "/api/public/me/estimates";
    let (_, started) = h
        .public_with(
            "POST",
            estimates,
            Some(json!({ "character_id": "ch-b1" })),
            &marked,
            202,
            "character_data#EstimateStarted",
        )
        .await;
    let job = format!("{estimates}/{}", s(&started["job"]["id"]));
    for _ in 0..3 {
        h.public("GET", &job, &read, 200, "character_data#EstimateJob")
            .await;
    }
    let (_, status) = h
        .public(
            "GET",
            &format!("{estimates}?character_id=ch-b1"),
            &read,
            200,
            "character_data#EstimateStatus",
        )
        .await;
    assert_eq!(status["job"]["state"], "done");
    let body = json!({ "character_id": "ch-b1", "refresh": true });
    let (code, _, limited) = h
        .send_with(true, "POST", estimates, Some(body), &marked)
        .await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        validator("public#MemberEstimateLimit").is_valid(&limited),
        "{limited}"
    );

    // The party on her run; other members' characters and runs are the 404.
    let (_, party) = h
        .public(
            "GET",
            "/api/public/runs/r-carling/characters",
            &read,
            200,
            "public#RunPartyCharacters",
        )
        .await;
    let slots = party["slots"].as_array().unwrap();
    assert!(slots.iter().filter(|slot| slot["you"] == true).count() == 1);
    assert!(
        slots.iter().filter(|slot| slot["you"] == false).all(
            |slot| slot["character"].is_null() || slot["character"]["level_source"] == "roster"
        )
    );
    for path in [
        "/api/public/me/characters/ch-s3/link",
        "/api/public/runs/nope/characters",
        "/api/public/me/estimates/est-seed-1",
    ] {
        h.public("GET", path, &read, 404, "").await;
    }
    h.public_with(
        "PATCH",
        "/api/public/me/characters/ch-s3",
        Some(json!({ "version": 1, "retired": true })),
        &key("e-2"),
        404,
        "",
    )
    .await;

    // Module off: the generic 404 before the session.
    h.control(false, "/__mock/modules", json!({ "character_data": false }))
        .await;
    h.public("GET", "/api/public/me/data", NONE, 404, "").await;
    h.public("GET", mine, &read, 200, "public#MyCharacters")
        .await;
    h.control(false, "/__mock/modules", json!({ "roster": false }))
        .await;
    h.public("GET", mine, NONE, 404, "").await;
    h.finish();
}
