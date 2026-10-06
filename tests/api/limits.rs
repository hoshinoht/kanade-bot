//! A8 Limits and the manual digest trigger: live projections remain admin-only,
//! and unsafe operations use the same CSRF/idempotency boundary as A4.

use serde_json::json;

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, public, request, send},
};

const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const LIMITS: &str = "/api/admin/limits";
const RESET: &str = "/api/admin/limits/windows/1001";
const DIGEST: &str = "/api/admin/digest";
const ERROR: &str = "error.json#/$defs/ApiError";
const TOKEN: &str = "break-glass-token-with-at-least-32-bytes!";

async fn write(
    reads: &Reads,
    path: &str,
    key: Option<&str>,
    body: Option<&str>,
) -> crate::support::Reply {
    let mut headers = vec![
        ORIGIN,
        ("Cookie", reads.cookie.as_str()),
        ("X-Kanade-CSRF", reads.csrf.as_str()),
    ];
    if let Some(key) = key {
        headers.push(("Idempotency-Key", key));
    }
    send(reads.admin, "DELETE", ADMIN_HOST, path, &headers, body).await
}

#[tokio::test]
async fn limits_project_live_groups_and_allowances_without_secrets() {
    let reads = Reads::new().await;
    let unauthenticated = request(reads.admin, "GET", ADMIN_HOST, LIMITS, &[]).await;
    assert_eq!(unauthenticated.status, 401);
    assert_valid(ERROR, "limits unauthenticated", &unauthenticated.json());

    let reply = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        LIMITS,
        &[("Cookie", &reads.cookie)],
    )
    .await;
    assert_eq!(reply.status, 200, "{}", reply.text());
    let value = reply.json();
    assert_valid("limits.json#/$defs/Limits", LIMITS, &value);
    assert!(value["groups"].is_array());
    // The page's "Updated …" line reads the server's clock, never the browser's.
    assert_eq!(value["generated_at"], "2026-09-29T04:00:00Z");
    assert!(
        value["allowances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["member"]["id"] == "1006")
    );
    assert!(!reply.dump().contains(TOKEN));
}

async fn allowance_row(reads: &Reads, member: &str) -> serde_json::Value {
    let limits = reads.read(LIMITS, "limits.json#/$defs/Limits").await;
    limits["allowances"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["member"]["id"] == member)
        .cloned()
        .unwrap()
}

/// `resets_at` is when the oldest counted answer leaves the window, against
/// the server clock paired with the snapshot (`generated_at`, 04:00:00Z at
/// monotonic second 0 here); null for staff and an empty window.
#[tokio::test]
async fn resets_at_is_when_the_oldest_answer_leaves_the_window() {
    let reads = Reads::new().await;
    // Eve (pilot), idle; Cara (staff) is exempt even with answers counted.
    assert_eq!(
        allowance_row(&reads, "1006").await["resets_at"],
        json!(null)
    );
    reads.chat.spend_at("1003", -10.0);
    let cara = allowance_row(&reads, "1003").await;
    assert_eq!(
        (cara["staff"].clone(), cara["used"].clone()),
        (json!(true), json!(0))
    );
    assert_eq!(cara["resets_at"], json!(null));

    // Several answers: the oldest one counts (default window 300 s).
    for at in [-100.0, -40.0, -5.5] {
        reads.chat.spend_at("1006", at);
    }
    let eve = allowance_row(&reads, "1006").await;
    assert_eq!(eve["used"], 3);
    assert_eq!(eve["resets_at"], "2026-09-29T04:03:20Z", "-100 + 300");

    // At the boundary the oldest has left, and the next oldest counts.
    *reads.chat.now.lock().unwrap() = 200.0;
    let eve = allowance_row(&reads, "1006").await;
    assert_eq!(eve["used"], 2);
    // The fixture's wall clock stays at 04:00:00 while monotonic time moved on.
    assert_eq!(eve["resets_at"], "2026-09-29T04:01:00Z", "-40 + 300 - 200");
    // A fraction of a second rounds up, never early.
    *reads.chat.now.lock().unwrap() = 260.0;
    let eve = allowance_row(&reads, "1006").await;
    assert_eq!(eve["used"], 1);
    assert_eq!(eve["resets_at"], "2026-09-29T04:00:35Z", "-5.5 + 300 - 260");
    *reads.chat.now.lock().unwrap() = 294.5;
    let eve = allowance_row(&reads, "1006").await;
    assert_eq!(
        (eve["used"].clone(), eve["resets_at"].clone()),
        (json!(0), json!(null))
    );
}

#[tokio::test]
async fn an_override_window_sets_resets_at() {
    let reads = Reads::new().await;
    reads.chat.override_allowance("1006", 20, 3_600_000);
    reads.chat.spend_at("1006", -1200.0);
    reads.chat.spend_at("1006", -60.0);
    let eve = allowance_row(&reads, "1006").await;
    assert_eq!(eve["override"], true);
    assert_eq!(eve["allowance"], json!({"count": 20, "per_s": 3600.0}));
    assert_eq!(eve["used"], 2);
    assert_eq!(eve["resets_at"], "2026-09-29T04:40:00Z", "-1200 + 3600");
}

#[tokio::test]
async fn resetting_a_window_needs_csrf_and_replays_once() {
    let reads = Reads::new().await;
    let unauthenticated = send(reads.admin, "DELETE", ADMIN_HOST, RESET, &[ORIGIN], None).await;
    assert_eq!(unauthenticated.status, 401);
    assert_valid(ERROR, "reset unauthenticated", &unauthenticated.json());
    let no_csrf = send(
        reads.admin,
        "DELETE",
        ADMIN_HOST,
        RESET,
        &[ORIGIN, ("Cookie", &reads.cookie)],
        None,
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));

    let first = write(&reads, RESET, Some("reset-1001"), None).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_valid("common.json#/$defs/Message", RESET, &first.json());
    let replay = write(&reads, RESET, Some("reset-1001"), None).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(reads.chat.resets.lock().unwrap().as_slice(), ["1001"]);
    let mismatch = write(
        &reads,
        "/api/admin/limits/windows/1002",
        Some("reset-1001"),
        None,
    )
    .await;
    assert_eq!(
        (mismatch.status, mismatch.api_error()),
        (422, "idempotency_mismatch".into())
    );
    for reply in [first, replay, mismatch] {
        assert!(!reply.dump().contains(TOKEN));
    }
}

async fn limit_records(reads: &Reads) -> Vec<serde_json::Value> {
    let page = reads
        .read("/api/admin/history", "history.json#/$defs/HistoryPage")
        .await;
    page["settings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["section"] == "limits")
        .cloned()
        .collect()
}

/// A clear is recorded in History once, with the actor and the window as
/// it was; a replayed key and an already-empty window record nothing.
#[tokio::test]
async fn an_effective_clear_is_recorded_once_and_no_ops_are_not() {
    let reads = Reads::new().await;
    reads.chat.spend("1001");
    reads.chat.spend("1001");
    let first = write(&reads, RESET, Some("clear-1"), None).await;
    assert_eq!(first.status, 200, "{}", first.text());
    let replay = write(&reads, RESET, Some("clear-1"), None).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    let empty = write(&reads, RESET, Some("clear-2"), None).await;
    assert_eq!(empty.status, 200, "{}", empty.text());
    let unkeyed = write(&reads, RESET, None, None).await;
    assert_eq!(unkeyed.status, 200, "{}", unkeyed.text());

    let records = limit_records(&reads).await;
    assert_eq!(records.len(), 1, "{records:?}");
    let record = &records[0];
    assert_eq!(record["actor"], json!({"kind": "admin", "id": "token"}));
    assert_eq!(record["surface"], "admin_portal");
    assert_eq!(record["revision"], 0);
    assert_eq!(record["at"], "2026-09-29T04:00:00+00:00");
    let values = record["values"].as_array().unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0]["key"], "window.1001");
    let from: serde_json::Value =
        serde_json::from_str(values[0]["from"].as_str().unwrap()).unwrap();
    let to: serde_json::Value = serde_json::from_str(values[0]["to"].as_str().unwrap()).unwrap();
    assert_eq!(from["member"], "Alice");
    assert_eq!(
        (from["used"].clone(), to["used"].clone()),
        (json!(2), json!(0))
    );
    assert_eq!(from["limit"], to["limit"]);
    assert_eq!(from["per_s"], to["per_s"]);
    assert!(from["per_s"].is_u64(), "whole seconds: {from}");

    // Another member's clear is its own record.
    reads.chat.spend("1002");
    let bob = write(
        &reads,
        "/api/admin/limits/windows/1002",
        Some("clear-3"),
        None,
    )
    .await;
    assert_eq!(bob.status, 200, "{}", bob.text());
    let records = limit_records(&reads).await;
    assert_eq!(records.len(), 2);
    assert_eq!(
        records[0]["values"][0]["key"], "window.1002",
        "newest first"
    );
}

/// Unkeyed clears of one window, sent together: one sees the answers and
/// records; the other finds the window empty.
#[tokio::test]
async fn concurrent_unkeyed_clears_record_once() {
    let reads = Reads::new().await;
    reads.chat.spend("1001");
    let (one, two) = tokio::join!(
        write(&reads, RESET, None, None),
        write(&reads, RESET, None, None)
    );
    assert_eq!((one.status, two.status), (200, 200));
    assert_eq!(limit_records(&reads).await.len(), 1);
    assert_eq!(reads.chat.resets.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn digest_uses_the_delivery_port_and_replays_once() {
    let reads = Reads::new().await;
    let body = json!({"week": "this", "channel_id": "kalos-four"}).to_string();
    let unauthenticated = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &[ORIGIN],
        Some(&body),
    )
    .await;
    assert_eq!(unauthenticated.status, 401);
    assert_valid(ERROR, "digest unauthenticated", &unauthenticated.json());
    let no_csrf = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &[ORIGIN, ("Cookie", &reads.cookie)],
        Some(&body),
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));

    let mut headers = vec![
        ORIGIN,
        ("Cookie", reads.cookie.as_str()),
        ("X-Kanade-CSRF", reads.csrf.as_str()),
        ("Idempotency-Key", "digest-now"),
    ];
    let first = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &headers,
        Some(&body),
    )
    .await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_valid("common.json#/$defs/Message", DIGEST, &first.json());
    let replay = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &headers,
        Some(&body),
    )
    .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(reads.digest_posts.lock().unwrap().len(), 1);
    let other = json!({"week": "next", "channel_id": "kalos-four"}).to_string();
    let mismatch = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &headers,
        Some(&other),
    )
    .await;
    assert_eq!(
        (mismatch.status, mismatch.api_error()),
        (422, "idempotency_mismatch".into())
    );
    headers.pop();
    let unknown = json!({"week": "this", "channel_id": "gone"}).to_string();
    let bad_channel = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &headers,
        Some(&unknown),
    )
    .await;
    assert_eq!(
        (bad_channel.status, bad_channel.api_error()),
        (422, "invalid".into())
    );
    for reply in [first, replay, mismatch, bad_channel] {
        assert!(!reply.dump().contains(TOKEN));
    }
}

#[tokio::test]
async fn digest_is_unavailable_without_live_delivery() {
    let reads = Reads::without_digest_delivery().await;
    let body = json!({"week": "this", "channel_id": "kalos-four"}).to_string();
    let reply = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        DIGEST,
        &[
            ORIGIN,
            ("Cookie", &reads.cookie),
            ("X-Kanade-CSRF", &reads.csrf),
        ],
        Some(&body),
    )
    .await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (503, "unavailable".into())
    );
    assert_valid(ERROR, "offline digest", &reply.json());
}

#[tokio::test]
async fn limits_and_digest_are_not_mounted_on_the_public_listener() {
    let fixture = Fixture::new();
    let address = public(&fixture.http()).await;
    for (method, path, body) in [
        ("GET", LIMITS, None),
        ("DELETE", RESET, None),
        (
            "POST",
            DIGEST,
            Some(r#"{"week":"this","channel_id":"kalos-four"}"#),
        ),
    ] {
        let reply = send(
            address,
            method,
            crate::support::PUBLIC_HOST,
            path,
            &[],
            body,
        )
        .await;
        assert_eq!(reply.status, 404, "{method} {path}: {}", reply.text());
        assert_valid(ERROR, path, &reply.json());
    }
}
