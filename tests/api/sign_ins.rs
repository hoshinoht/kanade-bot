//! History › Sign-ins (`GET /api/admin/history/sign-ins`): what admin sign-in
//! records through the stored audit sink, newest first, filtered, behind the
//! admin session. The sink writes off the request path, so reads wait for it.

use std::time::Duration;

use serde_json::Value;

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, request, send},
};

const PAGE: &str = "history.json#/$defs/SignInPage";
const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");

/// The page at `path` once `ready` holds for it (bounded).
async fn page_when(reads: &Reads, path: &str, ready: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..200 {
        let page = reads.read(path, PAGE).await;
        if ready(&page) {
            return page;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{path} never showed the expected rows");
}

fn rows(page: &Value) -> &Vec<Value> {
    page["rows"].as_array().unwrap()
}

#[tokio::test]
async fn sign_ins_are_stored_and_listed_newest_first_with_filters() {
    let reads = Reads::with_logins().await;
    // The harness signed in with the token; now a refused try, then Discord.
    let refused = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        "/api/admin/auth/token",
        &[ORIGIN],
        Some(r#"{"token":"a-wrong-token-that-is-long-enough-to-try"}"#),
    )
    .await;
    assert_eq!(refused.status, 401);
    reads.discord_session(1003, "Cara").await;

    let all = page_when(&reads, "/api/admin/history/sign-ins", |page| {
        rows(page).iter().any(|row| row["method"] == "discord")
    })
    .await;
    let seqs: Vec<i64> = rows(&all)
        .iter()
        .map(|row| row["seq"].as_i64().unwrap())
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] > pair[1]),
        "newest first: {seqs:?}"
    );
    assert!(rows(&all).iter().all(|row| row["realm"] == "admin"));
    let discord = rows(&all)
        .iter()
        .find(|row| row["event"] == "login_succeeded" && row["method"] == "discord")
        .unwrap();
    assert_eq!(discord["actor"], "discord:1003");
    assert!(
        discord["name"]
            .as_str()
            .is_some_and(|name| !name.is_empty())
    );
    // Admin rows keep the client address (the test client is loopback).
    assert_eq!(discord["client"], "127.0.0.1");
    assert!(
        rows(&all)
            .iter()
            .any(|row| row["event"] == "break_glass_used" && row["actor"] == "token"),
        "the harness's token sign-in is a break-glass use"
    );
    assert_eq!(all["next_before"], Value::Null);

    let refusals = reads
        .read("/api/admin/history/sign-ins?event=login_refused", PAGE)
        .await;
    let [refusal] = rows(&refusals).as_slice() else {
        panic!("one refusal: {refusals}");
    };
    assert_eq!(refusal["method"], "token");
    assert_eq!(refusal["reason"], "bad_token");
    let page = reads
        .read("/api/admin/history/sign-ins?actor=discord:1003", PAGE)
        .await;
    assert!(!rows(&page).is_empty());
    assert!(rows(&page).iter().all(|row| row["actor"] == "discord:1003"));
    let members = reads
        .read("/api/admin/history/sign-ins?realm=member", PAGE)
        .await;
    assert!(rows(&members).is_empty());

    let newest = seqs[0];
    let older = reads
        .read(
            &format!("/api/admin/history/sign-ins?before={newest}"),
            PAGE,
        )
        .await;
    assert!(
        rows(&older)
            .iter()
            .all(|row| row["seq"].as_i64().unwrap() < newest)
    );
    assert_eq!(rows(&older).len(), rows(&all).len() - 1);
}

#[tokio::test]
async fn sign_ins_refuse_bad_filters_and_need_an_admin_session() {
    let reads = Reads::new().await;
    for query in [
        "realm=public",
        "event=logout",
        "sort=asc",
        "before=0",
        "before=x",
        "realm=admin&realm=member",
    ] {
        let reply = request(
            reads.admin,
            "GET",
            ADMIN_HOST,
            &format!("/api/admin/history/sign-ins?{query}"),
            &[("Cookie", &reads.cookie)],
        )
        .await;
        assert_eq!(reply.status, 422, "{query}: {}", reply.text());
        assert_valid("error.json#/$defs/ApiError", query, &reply.json());
        assert_eq!(reply.api_error(), "invalid_filter", "{query}");
    }
    let (status, code) = reads.status("/api/admin/history/sign-ins", false).await;
    assert_eq!((status, code.as_str()), (401, "unauthenticated"));
}
