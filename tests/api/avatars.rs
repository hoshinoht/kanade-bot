//! Member and admin portraits: `/api/admin/members/{id}/avatar` and
//! `/api/admin/me/avatar` need an admin session, are never mounted on the
//! public listener, cache the CDN image by hash (ETag/304), and answer the
//! monogram when there is no avatar or the CDN fails.

use kanade::domain::members::MemberStore;

use crate::{
    reads::{PNG, Reads, avatar_hash},
    support::{ADMIN_HOST, Fixture, PUBLIC_HOST, Reply, public, request},
};

const ALICE: &str = "/api/admin/members/1001/avatar";
const BOB: &str = "/api/admin/members/1002/avatar";
const CARA: &str = "/api/admin/members/1003/avatar";
const DAN: &str = "/api/admin/members/1004/avatar";
const ME: &str = "/api/admin/me/avatar";
const ERROR: &str = "error.json#/$defs/ApiError";

async fn get(reads: &Reads, path: &str, extra: &[(&str, &str)]) -> Reply {
    let mut headers = vec![("Cookie", reads.cookie.as_str())];
    headers.extend_from_slice(extra);
    request(reads.admin, "GET", ADMIN_HOST, path, &headers).await
}

fn cached(reads: &Reads) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(&reads.avatar_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

fn assert_portrait_headers(reply: &Reply) {
    assert_eq!(reply.header("cache-control"), Some("private, no-cache"));
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert!(reply.header("etag").is_some());
}

#[tokio::test]
async fn portraits_need_an_admin_session_and_are_admin_only() {
    let reads = Reads::new().await;
    for path in [ALICE, ME] {
        let reply = request(reads.admin, "GET", ADMIN_HOST, path, &[]).await;
        assert_eq!(reply.status, 401, "{path}");
        crate::schemas::assert_valid(ERROR, path, &reply.json());
        assert_eq!(reply.header("cache-control"), Some("no-store"));
    }
    assert!(
        reads.cdn.calls.lock().unwrap().is_empty(),
        "no fetch unsigned"
    );

    let fixture = Fixture::new();
    let address = public(&fixture.http()).await;
    for path in [ALICE, ME] {
        let reply = request(address, "GET", PUBLIC_HOST, path, &[]).await;
        assert_eq!(reply.status, 404, "{path}");
        crate::schemas::assert_valid(ERROR, path, &reply.json());
    }
}

#[tokio::test]
async fn a_member_avatar_is_fetched_once_cached_by_hash_and_revalidated() {
    let reads = Reads::new().await;
    let first = get(&reads, ALICE, &[]).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(first.header("content-type"), Some("image/png"));
    assert_portrait_headers(&first);
    let path = format!(
        "/guilds/900/users/1001/avatars/{}.png?size=128",
        avatar_hash(1)
    );
    assert_eq!(
        first.body,
        [PNG, path.as_bytes()].concat(),
        "the guild avatar"
    );
    assert_eq!(cached(&reads), [format!("1001-{}.png", avatar_hash(1))]);

    let hit = get(&reads, ALICE, &[]).await;
    assert_eq!((hit.status, &hit.body), (200, &first.body));
    assert_eq!(hit.header("etag"), first.header("etag"));
    let etag = first.header("etag").unwrap();
    let fresh = get(&reads, ALICE, &[("If-None-Match", etag)]).await;
    assert_eq!(fresh.status, 304);
    assert!(fresh.body.is_empty());
    assert_eq!(fresh.header("etag"), Some(etag));
    assert_eq!(fresh.header("cache-control"), Some("private, no-cache"));
    assert_eq!(
        reads.cdn.calls.lock().unwrap().as_slice(),
        [path],
        "one CDN fetch: later answers come from the cache"
    );

    let bob = get(&reads, BOB, &[]).await;
    assert_eq!(bob.header("content-type"), Some("image/png"));
    assert!(
        bob.text()
            .contains(&format!("/avatars/1002/{}.png", avatar_hash(2)))
    );
}

#[tokio::test]
async fn no_avatar_or_a_failed_fetch_is_the_monogram_and_unknown_ids_are_404() {
    let reads = Reads::new().await;
    let cara = get(&reads, CARA, &[]).await;
    assert_eq!(cara.status, 200);
    assert_eq!(cara.header("content-type"), Some("image/svg+xml"));
    assert_portrait_headers(&cara);
    assert!(cara.text().contains(">C</text>"), "{}", cara.text());

    for _ in 0..2 {
        let dan = get(&reads, DAN, &[]).await;
        assert_eq!(dan.status, 200);
        assert_eq!(dan.header("content-type"), Some("image/svg+xml"));
        assert!(dan.text().contains(">D</text>"));
    }
    assert_eq!(
        reads.cdn.calls.lock().unwrap().len(),
        1,
        "a failed fetch is not retried at once"
    );
    assert!(cached(&reads).is_empty(), "nothing cached from a failure");

    for path in [
        "/api/admin/members/9999/avatar",
        "/api/admin/members/abc/avatar",
        "/api/admin/members/..%2F1001/avatar",
    ] {
        let reply = get(&reads, path, &[]).await;
        assert_eq!(reply.status, 404, "{path}");
    }
}

#[tokio::test]
async fn the_signed_in_admins_portrait_uses_the_hash_stored_at_sign_in() {
    let reads = Reads::with_logins().await;
    let token = get(&reads, ME, &[]).await;
    assert_eq!(token.status, 200);
    assert_eq!(token.header("content-type"), Some("image/svg+xml"));
    assert!(
        token.text().contains(">B</text>"),
        "Break-glass token monogram"
    );

    // Cara is staff and the gateway shows no avatar for her: the sign-in's hash.
    let mut cara = reads.store.load_member("1003").await.unwrap().unwrap();
    cara.roles = vec!["20".into()];
    reads.store.put_member(cara).await.unwrap();
    let (cookie, _) = reads.discord_session(1003, "Cara").await;
    let me = request(reads.admin, "GET", ADMIN_HOST, ME, &[("Cookie", &cookie)]).await;
    assert_eq!(me.status, 200, "{}", me.text());
    assert_eq!(me.header("content-type"), Some("image/png"));
    assert_portrait_headers(&me);
    let path = format!("/avatars/1003/{}.png?size=128", avatar_hash(1003));
    assert_eq!(me.body, [PNG, path.as_bytes()].concat());
    assert_eq!(cached(&reads), [format!("1003-{}.png", avatar_hash(1003))]);
}
