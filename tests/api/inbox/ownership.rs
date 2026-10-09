//! The Inbox Ownership tab: open weekly-timing ownership requests listed
//! oldest first (expired ones left to the tick), counted in the summary's
//! inbox, and accepted or declined by any admin session as staff.

use kanade::domain::ownership::{OwnerRequest, OwnerRequestStatus, OwnerRequestStore};

use super::*;

const REQUESTS: &str = "inbox.json#/$defs/OwnershipRequests";
const LIST: &str = "/api/admin/inbox/ownership";

fn decision(id: &str, action: &str) -> String {
    format!("/api/admin/inbox/ownership/{id}/{action}")
}

async fn ask(reads: &Reads, id: &str, requester: &str, at: DateTime<Utc>) {
    let request = OwnerRequest::open(
        id.into(),
        "f-kalos".into(),
        requester.into(),
        Some("kalos-four".into()),
        at,
    );
    OwnerRequestStore::create_owner_request(&*reads.store, request)
        .await
        .unwrap();
}

async fn get(reads: &Reads, path: &str) -> Value {
    let reply = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        path,
        &[("Cookie", reads.cookie.as_str())],
    )
    .await;
    assert_eq!(reply.status, 200, "{path}: {}", reply.text());
    reply.json()
}

async fn listed(reads: &Reads) -> Vec<Value> {
    let value = get(reads, LIST).await;
    assert_valid(REQUESTS, LIST, &value);
    value.as_array().unwrap().clone()
}

async fn inbox_count(reads: &Reads) -> u64 {
    get(reads, "/api/admin/summary").await["inbox"]
        .as_u64()
        .unwrap()
}

/// `(owner_id, owner_pinned)` of the seeded timing as `/api/admin/fixed` shows it.
async fn owner(reads: &Reads) -> (String, bool) {
    let rows = get(reads, "/api/admin/fixed").await;
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "f-kalos")
        .unwrap()
        .clone();
    (
        row["owner_id"].as_str().unwrap().to_owned(),
        row["owner_pinned"].as_bool().unwrap(),
    )
}

async fn status(reads: &Reads, id: &str) -> OwnerRequestStatus {
    OwnerRequestStore::owner_request(&*reads.store, id)
        .await
        .unwrap()
        .unwrap()
        .status
}

async fn post(
    reads: &Reads,
    session: &(String, String),
    path: &str,
    extra: &[(&str, &str)],
) -> Reply {
    let mut headers = vec![
        ("Cookie", session.0.as_str()),
        ORIGIN,
        ("X-Kanade-CSRF", session.1.as_str()),
    ];
    headers.extend_from_slice(extra);
    send(reads.admin, "POST", ADMIN_HOST, path, &headers, Some("{}")).await
}

async fn head(reads: &Reads) -> u64 {
    reads.store.history_head().await.unwrap().seq
}

#[tokio::test]
async fn ownership_requests_are_listed_counted_and_decided_by_any_admin_session() {
    let reads = Reads::with_logins().await;
    let token = (reads.cookie.clone(), reads.csrf.clone());
    let discord = reads.discord_session(1003, "Cara").await;
    let base = inbox_count(&reads).await;
    assert!(listed(&reads).await.is_empty());

    // Bob and Cara (party members) ask; Dan's request expired an hour ago.
    ask(&reads, "own-bob", "1002", utc(9, 29, 2, 0)).await;
    ask(&reads, "own-cara", "1003", utc(9, 29, 3, 0)).await;
    ask(&reads, "own-old", "1004", utc(9, 28, 3, 0)).await;
    let items = listed(&reads).await;
    let ids: Vec<&str> = items.iter().map(|i| i["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        ["own-bob", "own-cara"],
        "open, unexpired, oldest first"
    );
    let bob = &items[0];
    assert_eq!(bob["fixed_id"], "f-kalos");
    assert_eq!(bob["weekday"], 1);
    assert_eq!(bob["weekday_name"], "Tuesday");
    assert_eq!(bob["time"], "22:00");
    assert_eq!(bob["bosses"][0]["token"], "XKalos");
    assert_eq!(bob["requester"]["id"], "1002");
    assert_eq!(bob["owner"], json!({ "id": "1001", "name": "Alice" }));
    assert!(bob["channel"].is_string());
    assert_eq!(bob["created_at"], "2026-09-29T02:00:00Z");
    assert_eq!(bob["expires_at"], "2026-09-30T02:00:00Z");
    assert_eq!(
        inbox_count(&reads).await,
        base + 2,
        "the nav badge counts them"
    );

    // Refusals write nothing.
    let before = head(&reads).await;
    let no_csrf = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        &decision("own-bob", "accept"),
        &[("Cookie", reads.cookie.as_str()), ORIGIN],
        Some("{}"),
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    for action in ["accept", "decline"] {
        let reply = post(&reads, &token, &decision("nope", action), &[]).await;
        assert_eq!(refused(&reply), code(404, "not_found"), "{action}");
    }
    let bad_key = [("Idempotency-Key", "bad key")];
    assert_eq!(
        refused(&post(&reads, &token, &decision("own-bob", "accept"), &bad_key).await),
        code(400, "invalid_idempotency_key")
    );
    // Expired: the tick closes it; it is never accepted here.
    let expired = post(&reads, &token, &decision("own-old", "accept"), &[]).await;
    assert_eq!(refused(&expired), code(409, "conflicts"));
    assert_eq!(expired.json()["message"], "That request has expired.");
    assert_eq!(head(&reads).await, before);
    assert_eq!(status(&reads, "own-bob").await, OwnerRequestStatus::Open);

    // A token session declines; the owner stays; a retry answers the same.
    let key = [("Idempotency-Key", "decline-cara-1")];
    let first = ok(&post(&reads, &token, &decision("own-cara", "decline"), &key).await);
    assert_eq!(
        status(&reads, "own-cara").await,
        OwnerRequestStatus::Declined
    );
    assert_eq!(owner(&reads).await, ("1001".into(), false));
    let again = ok(&post(&reads, &token, &decision("own-cara", "decline"), &key).await);
    assert_eq!(first, again, "a replayed decline answers the first result");
    assert_eq!(head(&reads).await, before);
    // Closed: it cannot be accepted now.
    let closed = post(&reads, &token, &decision("own-cara", "accept"), &[]).await;
    assert_eq!(refused(&closed), code(409, "conflicts"));
    assert_eq!(
        closed.json()["message"],
        "That request has already been decided."
    );

    // A Discord admin accepts: Bob is pinned owner in one record; a retry
    // answers the same and writes nothing more.
    let key = [("Idempotency-Key", "accept-bob-1")];
    let first = ok(&post(&reads, &discord, &decision("own-bob", "accept"), &key).await);
    assert_eq!(
        status(&reads, "own-bob").await,
        OwnerRequestStatus::Accepted
    );
    assert_eq!(owner(&reads).await, ("1002".into(), true));
    let after = head(&reads).await;
    assert_eq!(after, before + 1, "one pinning record");
    let again = ok(&post(&reads, &discord, &decision("own-bob", "accept"), &key).await);
    assert_eq!(first, again, "a replayed accept answers the first result");
    assert_eq!(head(&reads).await, after);
    // Another admin's different decision on it is refused.
    assert_eq!(
        refused(&post(&reads, &token, &decision("own-bob", "decline"), &[]).await),
        code(409, "conflicts")
    );
    assert!(listed(&reads).await.is_empty());
    assert_eq!(inbox_count(&reads).await, base);
}

#[tokio::test]
async fn a_requester_no_longer_on_the_party_cannot_be_accepted() {
    let reads = Reads::with_logins().await;
    let token = (reads.cookie.clone(), reads.csrf.clone());
    // Dan asked while on the party, then left it.
    ask(&reads, "own-dan", "1004", utc(9, 29, 3, 0)).await;
    let before = head(&reads).await;
    let reply = post(&reads, &token, &decision("own-dan", "accept"), &[]).await;
    assert_eq!(refused(&reply), code(409, "conflicts"));
    assert_eq!(
        reply.json()["message"],
        "Ownership only moves between members of the party."
    );
    assert_eq!(head(&reads).await, before);
    assert_eq!(owner(&reads).await, ("1001".into(), false));
    // Declining still closes it.
    ok(&post(&reads, &token, &decision("own-dan", "decline"), &[]).await);
    assert_eq!(
        status(&reads, "own-dan").await,
        OwnerRequestStatus::Declined
    );
}
