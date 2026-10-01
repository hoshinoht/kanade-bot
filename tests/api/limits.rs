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
    assert!(
        value["allowances"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["member"]["id"] == "1006")
    );
    assert!(!reply.dump().contains(TOKEN));
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
