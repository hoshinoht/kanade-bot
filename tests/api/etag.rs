//! Conditional admin reads: a strong `ETag` on every JSON `GET`, `304` with
//! the same headers and no body when `If-None-Match` names it, and a new tag
//! once the data changes.

use serde_json::json;

use crate::{
    reads::Reads,
    support::{ADMIN_HOST, Reply, request},
};

async fn get(reads: &Reads, path: &str, tag: Option<&str>) -> Reply {
    let mut headers = vec![("Cookie", reads.cookie.as_str())];
    if let Some(tag) = tag {
        headers.push(("If-None-Match", tag));
    }
    request(reads.admin, "GET", ADMIN_HOST, path, &headers).await
}

/// Every header but the ones that describe the body.
fn shared(reply: &Reply) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = reply
        .headers
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "content-length" | "content-type" | "date"))
        .cloned()
        .collect();
    headers.sort();
    headers
}

#[tokio::test]
async fn hot_reads_revalidate_with_strong_tags() {
    let reads = Reads::new().await;
    for path in [
        "/api/admin/week",
        "/api/admin/stats",
        "/api/admin/summary",
        "/api/admin/inbox",
        "/api/admin/reminders",
        "/api/admin/members",
        "/api/admin/chat",
        "/api/admin/extractions",
        "/api/admin/rewrites",
    ] {
        let first = get(&reads, path, None).await;
        assert_eq!(first.status, 200, "{path}: {}", first.text());
        let tag = first
            .header("etag")
            .unwrap_or_else(|| panic!("{path} tagged"))
            .to_owned();
        assert!(
            tag.starts_with('"') && !tag.starts_with("W/"),
            "strong: {tag}"
        );

        let same = get(&reads, path, Some(&tag)).await;
        assert_eq!(same.status, 304, "{path}");
        assert!(same.body.is_empty(), "{path}: no body");
        assert_eq!(same.header("etag"), Some(tag.as_str()));
        assert_eq!(same.header("cache-control"), Some("no-store"));
        assert_eq!(shared(&same), shared(&first), "{path}: the 200's headers");

        let listed = get(&reads, path, Some(&format!("\"other\", W/{tag}"))).await;
        assert_eq!(listed.status, 304, "{path}: a listed weak form matches");
        let other = get(&reads, path, Some("\"other\"")).await;
        assert_eq!((other.status, other.body), (200, first.body), "{path}");
    }
}

#[tokio::test]
async fn a_change_gets_a_new_tag_and_writes_and_errors_are_untagged() {
    let reads = Reads::new().await;
    let before = get(&reads, "/api/admin/week", None).await;
    let tag = before.header("etag").unwrap().to_owned();
    let version = reads.version().await;
    let moved = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 6, "time": "21:00", "version": version}),
            &[],
        )
        .await;
    assert_eq!(moved.status, 200);
    assert_eq!(moved.header("etag"), None, "writes are not tagged");
    let after = get(&reads, "/api/admin/week", Some(&tag)).await;
    assert_eq!(after.status, 200, "changed: the full week again");
    assert_ne!(after.header("etag"), Some(tag.as_str()));

    let missing = get(&reads, "/api/admin/chat/nope", None).await;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.header("etag"), None);
    let signed_out = request(reads.admin, "GET", ADMIN_HOST, "/api/admin/week", &[]).await;
    assert_eq!((signed_out.status, signed_out.header("etag")), (401, None));
}
