use chrono::Utc;
use kanade::{
    api::dto::rescan::{BACKFILL_FAILED, CHANNEL_FAILED, OTHER_FAILED},
    domain::{model_log::RescanStatus, schedule::utc_instant},
    extract::{
        rescan::{ResolvedWindow, resolve_window},
        window::window_since,
    },
};
use serde_json::{Value, json};

use super::{ERROR, Logs, ORIGIN, policy, utc};
use crate::{
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, request, send},
};

const JOB: &str = "extractions.json#/$defs/RescanJob";

fn job(reply: &Reply, what: &str) -> Value {
    assert_eq!(reply.status, 200, "{what}: {}", reply.text());
    let value = reply.json();
    assert_valid(JOB, what, &value);
    value
}

fn refused(reply: &Reply, status: u16, code: &str) {
    assert_eq!(reply.status, status, "{}", reply.text());
    assert_valid(ERROR, code, &reply.json());
    assert_eq!(reply.api_error(), code);
}

async fn start(logs: &Logs, key: Option<&str>, body: &str) -> Reply {
    logs.write("POST", "/api/admin/rescan", key, Some(body))
        .await
}

async fn poll(logs: &Logs, id: &str) -> Value {
    job(&logs.get(&format!("/api/admin/rescan/{id}")).await, "poll")
}

/// `[started_at, messages, messages_total]`.
fn progress(job: &Value) -> Value {
    json!([job["started_at"], job["messages"], job["messages_total"]])
}

fn states(job: &Value) -> Vec<&str> {
    job["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|channel| channel["state"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn rescan_targets_are_the_watched_channels_only() {
    let logs = Logs::new().await;
    let reply = logs.get("/api/admin/rescan/targets").await;
    assert_eq!(reply.status, 200);
    let targets = reply.json();
    assert_valid("common.json#/$defs/Channels", "targets", &targets);
    assert_eq!(
        targets,
        json!([
            {"id": "kalos-four", "name": "#kalos-four"},
            {"id": "limbo-trio", "name": "#limbo-trio"},
        ])
    );
}

#[tokio::test]
async fn a_rescan_is_queued_at_once_then_progresses_per_channel_with_unread_counts() {
    let logs = Logs::new().await;
    let fake = &logs.reads.rescans;
    fake.results.lock().unwrap().insert(
        "limbo-trio".into(),
        json!({"channel_id": "limbo-trio", "name": "#limbo-trio", "gated": 5, "proposals": 2,
        "unread": 2, "errors": [
            "backfill: GET /secret/path answered 500",
            "2 message(s) not read: the model kept turning the rescan away",
            "store: /private/var/db is locked",
        ]}),
    );
    fake.expected
        .lock()
        .unwrap()
        .extend([("kalos-four".to_owned(), 4), ("limbo-trio".to_owned(), 6)]);
    let body = r#"{"channels":["kalos-four","limbo-trio","kalos-four"],"window":"since_reset"}"#;
    let first = job(&start(&logs, Some("scan-1"), body).await, "submit");
    // Answered before the runner took a single step.
    assert_eq!(first["state"], "running");
    assert_eq!(progress(&first), json!([null, 0, null]), "not started");
    assert_eq!(first["window"], "since_reset");
    assert_eq!(states(&first), ["queued", "queued"]);
    assert_eq!(
        (first["proposals"].clone(), first["unread"].clone()),
        (json!(0), json!(0))
    );
    let id = first["id"].as_str().unwrap().to_owned();
    {
        let requests = fake.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(
            request.channels,
            ["kalos-four", "limbo-trio"],
            "deduplicated"
        );
        assert_eq!(request.window, "since_reset");
        assert_eq!(request.source, "portal");
        assert!(!request.automated);
        assert_eq!(request.requested_by.as_deref(), Some("admin:token"));
    }

    fake.step(&id).await;
    let started = poll(&logs, &id).await;
    assert_eq!(states(&started), ["reading", "queued"]);
    // The pinned clock; the total is each channel's count at the start.
    assert_eq!(progress(&started), json!(["2026-09-29T04:00:00Z", 0, 10]));
    fake.step(&id).await;
    let midway = poll(&logs, &id).await;
    assert_eq!(states(&midway), ["done", "reading"]);
    assert_eq!(midway["channels"][0]["messages"], 3);
    // A read channel counts what it read in place of its expected count.
    assert_eq!(progress(&midway), json!(["2026-09-29T04:00:00Z", 3, 9]));
    fake.step(&id).await;
    let done = logs.get(&format!("/api/admin/rescan/{id}")).await;
    assert!(!done.text().contains("secret") && !done.text().contains("/private"));
    let done = job(&done, "done");
    assert_eq!(done["state"], "done");
    assert_eq!(progress(&done), json!(["2026-09-29T04:00:00Z", 8, 8]));
    assert_eq!(done["proposals"], 3);
    assert_eq!(done["unread"], 2);
    assert_eq!(
        done["channels"][1],
        json!({
            "id": "limbo-trio", "name": "#limbo-trio", "state": "done", "messages": 5, "unread": 2,
            "errors": [
                "2 message(s) not read: the model kept turning the rescan away.",
                BACKFILL_FAILED,
                OTHER_FAILED,
            ],
        })
    );
    assert_eq!(done["channels"][0]["errors"], json!([]));
}

#[tokio::test]
async fn a_submit_retry_with_its_key_replays_the_job_and_another_request_is_a_mismatch() {
    let logs = Logs::new().await;
    let fake = &logs.reads.rescans;
    let body = r#"{"channels":["kalos-four","limbo-trio"],"window":"week"}"#;
    let first = job(&start(&logs, Some("scan-1"), body).await, "first");
    let id = first["id"].as_str().unwrap().to_owned();
    fake.step(&id).await;
    // Same channels in another order are the same request.
    let again = r#"{"channels":["limbo-trio","kalos-four"],"window":"week"}"#;
    let replay = job(&start(&logs, Some("scan-1"), again).await, "replay");
    assert_eq!(replay["id"], id.as_str());
    assert_eq!(
        states(&replay),
        ["reading", "queued"],
        "the job's current state"
    );
    assert_eq!(fake.requests.lock().unwrap().len(), 1, "submitted once");

    let other = r#"{"channels":["kalos-four"],"window":"two_weeks"}"#;
    refused(
        &start(&logs, Some("scan-1"), other).await,
        422,
        "idempotency_mismatch",
    );
    refused(
        &start(&logs, Some("bad key!"), body).await,
        400,
        "invalid_idempotency_key",
    );
    let fresh = job(&start(&logs, None, other).await, "unkeyed");
    assert_ne!(fresh["id"], id.as_str());
    assert_eq!(fresh["window"], "two_weeks");
    assert_eq!(fake.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancelling_is_immediate_for_queued_jobs_stops_running_ones_and_is_safe_to_repeat() {
    let logs = Logs::new().await;
    let fake = &logs.reads.rescans;
    let queued = job(
        &start(
            &logs,
            None,
            r#"{"channels":["kalos-four"],"window":"week"}"#,
        )
        .await,
        "queued",
    );
    let queued_id = queued["id"].as_str().unwrap().to_owned();
    let path = format!("/api/admin/rescan/{queued_id}");
    let cancelled = job(
        &logs.write("DELETE", &path, Some("stop-1"), None).await,
        "cancel",
    );
    assert_eq!(cancelled["state"], "cancelled");
    for key in [None, Some("stop-1")] {
        let again = job(&logs.write("DELETE", &path, key, None).await, "repeat");
        assert_eq!(again["state"], "cancelled");
    }

    let running = job(
        &start(
            &logs,
            None,
            r#"{"channels":["kalos-four","limbo-trio"],"window":"two_weeks"}"#,
        )
        .await,
        "running",
    );
    let id = running["id"].as_str().unwrap().to_owned();
    fake.step(&id).await;
    let path = format!("/api/admin/rescan/{id}");
    refused(
        &logs.write("DELETE", &path, Some("stop-1"), None).await,
        422,
        "idempotency_mismatch",
    );
    let stopping = job(
        &logs.write("DELETE", &path, Some("stop-2"), None).await,
        "stopping",
    );
    // It ends cancelled once the read in flight finishes.
    assert_eq!(stopping["state"], "cancelled");
    assert_eq!(fake.status(&id), RescanStatus::Running);
    fake.step(&id).await;
    assert_eq!(fake.status(&id), RescanStatus::Cancelled);
    let after = poll(&logs, &id).await;
    assert_eq!(after["state"], "cancelled");
    assert_eq!(states(&after), ["queued", "queued"]);
    assert_eq!(
        (after["messages"].clone(), after["messages_total"].clone()),
        (json!(0), json!(0)),
        "a stopped job's total is what it read"
    );
    let repeat = job(&logs.write("DELETE", &path, None, None).await, "repeat");
    assert_eq!(repeat["state"], "cancelled");

    // A finished job is answered as it is.
    let finished = job(
        &start(
            &logs,
            None,
            r#"{"channels":["kalos-four"],"window":"week"}"#,
        )
        .await,
        "finished",
    );
    let finished_id = finished["id"].as_str().unwrap().to_owned();
    fake.step(&finished_id).await;
    fake.step(&finished_id).await;
    let path = format!("/api/admin/rescan/{finished_id}");
    assert_eq!(
        job(&logs.write("DELETE", &path, None, None).await, "done")["state"],
        "done"
    );

    refused(
        &logs
            .write("DELETE", "/api/admin/rescan/nope", None, None)
            .await,
        404,
        "not_found",
    );
    refused(&logs.get("/api/admin/rescan/nope").await, 404, "not_found");
}

#[tokio::test]
async fn a_channel_that_could_not_be_read_is_done_with_an_error() {
    let logs = Logs::new().await;
    let fake = &logs.reads.rescans;
    fake.failing.lock().unwrap().insert("kalos-four".into());
    // kalos-four could not be counted either.
    fake.expected.lock().unwrap().insert("limbo-trio".into(), 7);
    let started = job(
        &start(
            &logs,
            None,
            r#"{"channels":["kalos-four","limbo-trio"],"window":"week"}"#,
        )
        .await,
        "start",
    );
    let id = started["id"].as_str().unwrap().to_owned();
    fake.step(&id).await;
    let uncounted = poll(&logs, &id).await;
    assert_eq!(states(&uncounted), ["reading", "queued"]);
    assert_eq!(uncounted["messages_total"], Value::Null, "no total to show");
    fake.step(&id).await;
    let midway = poll(&logs, &id).await;
    assert_eq!(states(&midway), ["done", "reading"]);
    assert_eq!(midway["channels"][0]["errors"], json!([CHANNEL_FAILED]));
    // The failed channel read nothing, so it adds nothing.
    assert_eq!(
        (midway["messages"].clone(), midway["messages_total"].clone()),
        (json!(0), json!(7))
    );
    fake.step(&id).await;
    let done = poll(&logs, &id).await;
    assert_eq!(done["state"], "done");
    assert_eq!(done["proposals"], 1);
    assert_eq!(
        (done["messages"].clone(), done["messages_total"].clone()),
        (json!(3), json!(3))
    );
}

#[tokio::test]
async fn rescan_requests_are_validated() {
    let logs = Logs::new().await;
    for (body, status, code) in [
        (r#"{"channels":["star"],"window":"week"}"#, 422, "invalid"),
        (
            r#"{"channels":["star","nowhere"],"window":"week"}"#,
            422,
            "invalid",
        ),
        (
            r#"{"channels":["kalos-four","nowhere"],"window":"week"}"#,
            422,
            "invalid",
        ),
        (r#"{"channels":[],"window":"week"}"#, 422, "invalid"),
        (
            r#"{"channels":["kalos-four"],"window":"month"}"#,
            422,
            "invalid",
        ),
        (
            r#"{"channels":["kalos-four"],"window":"2weeks"}"#,
            422,
            "invalid",
        ),
        (r#"{"channels":["kalos-four"]}"#, 400, "invalid_body"),
        (
            r#"{"channels":["kalos-four"],"window":"week","force":true}"#,
            400,
            "invalid_body",
        ),
    ] {
        let reply = start(&logs, None, body).await;
        refused(&reply, status, code);
    }
    let reply = start(&logs, None, r#"{"channels":["star"],"window":"week"}"#).await;
    assert!(
        reply.text().contains("#star is not watched"),
        "{}",
        reply.text()
    );
    assert!(logs.reads.rescans.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_rescan_while_extraction_is_off_says_how_to_switch_it_on() {
    let logs = Logs::new().await;
    *logs.reads.rescans.off.lock().unwrap() = true;
    let reply = start(
        &logs,
        None,
        r#"{"channels":["kalos-four"],"window":"since_reset"}"#,
    )
    .await;
    refused(&reply, 409, "extraction_off");
    assert!(
        reply.text().contains("Config → Watching"),
        "{}",
        reply.text()
    );
}

#[tokio::test]
async fn every_log_and_rescan_route_needs_a_session_and_writes_need_csrf() {
    let logs = Logs::new().await;
    let admin = logs.reads.admin;
    for (method, path) in [
        ("GET", "/api/admin/chat"),
        ("GET", "/api/admin/chat/c-answer"),
        ("GET", "/api/admin/extractions"),
        ("GET", "/api/admin/extractions/x-new"),
        ("GET", "/api/admin/rescan/targets"),
        ("GET", "/api/admin/rescan/job-1"),
        ("POST", "/api/admin/rescan"),
        ("DELETE", "/api/admin/rescan/job-1"),
    ] {
        let reply = send(admin, method, ADMIN_HOST, path, &[ORIGIN], Some("{}")).await;
        refused(&reply, 401, "unauthenticated");
    }
    let body = r#"{"channels":["kalos-four"],"window":"week"}"#;
    for (method, path) in [
        ("POST", "/api/admin/rescan"),
        ("DELETE", "/api/admin/rescan/job-1"),
    ] {
        let reply = send(
            admin,
            method,
            ADMIN_HOST,
            path,
            &[ORIGIN, ("Cookie", &logs.reads.cookie)],
            Some(body),
        )
        .await;
        refused(&reply, 403, "csrf");
    }
    // The bearer (CLI) needs no CSRF token.
    let bearer = request(
        admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/rescan/targets",
        &[(
            "Authorization",
            "Bearer break-glass-token-with-at-least-32-bytes!",
        )],
    )
    .await;
    assert_eq!(bearer.status, 200);
    assert!(logs.reads.rescans.requests.lock().unwrap().is_empty());
}

#[test]
fn since_reset_reads_from_the_current_boss_week_start_and_never_widens() {
    assert_eq!(
        resolve_window("since_reset", false).unwrap(),
        ResolvedWindow {
            key: "week",
            may_widen: false
        }
    );
    let policy = policy();
    let now = utc(9, 29, 4, 0);
    let since = window_since(
        "week",
        policy.zone(),
        policy.reset_weekday,
        policy.reset_time,
        &now.fixed_offset(),
    )
    .unwrap()
    .with_timezone(&Utc);
    let week_start = utc_instant(&policy.week_of(&now).unwrap()).unwrap();
    assert_eq!(since, week_start);
    assert_eq!(since, utc(9, 23, 16, 0), "Thu 24 Sep 00:00 KL");
}
