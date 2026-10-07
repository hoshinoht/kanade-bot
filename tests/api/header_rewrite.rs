//! `POST /api/admin/headers/rewrite`: the admin session and CSRF boundary,
//! `202` at once from the delivery port, idempotent replay, and refusals
//! while a run is active, without rewrites set up or offline.

use kanade::bot::delivery::ManualStart;

use crate::{
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, send},
};

const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const PATH: &str = "/api/admin/headers/rewrite";
const ERROR: &str = "error.json#/$defs/ApiError";
const MESSAGE: &str = "common.json#/$defs/Message";

async fn post(reads: &Reads, key: Option<&str>) -> crate::support::Reply {
    let mut headers = vec![
        ORIGIN,
        ("Cookie", reads.cookie.as_str()),
        ("X-Kanade-CSRF", reads.csrf.as_str()),
    ];
    if let Some(key) = key {
        headers.push(("Idempotency-Key", key));
    }
    send(reads.admin, "POST", ADMIN_HOST, PATH, &headers, None).await
}

#[tokio::test]
async fn needs_an_admin_session_and_csrf_then_answers_at_once_and_replays() {
    let reads = Reads::new().await;
    let unauthenticated = send(reads.admin, "POST", ADMIN_HOST, PATH, &[ORIGIN], None).await;
    assert_eq!(unauthenticated.status, 401);
    assert_valid(ERROR, "rewrite unauthenticated", &unauthenticated.json());
    let no_csrf = send(
        reads.admin,
        "POST",
        ADMIN_HOST,
        PATH,
        &[ORIGIN, ("Cookie", &reads.cookie)],
        None,
    )
    .await;
    assert_eq!((no_csrf.status, no_csrf.api_error()), (403, "csrf".into()));
    assert!(reads.header_rewrites.lock().unwrap().is_empty());

    let first = post(&reads, Some("rewrite-now")).await;
    assert_eq!(first.status, 202, "{}", first.text());
    assert_valid(MESSAGE, PATH, &first.json());
    assert_eq!(
        first.json()["message"],
        "Rewriting 3 header(s); see the Rewrites log."
    );
    let replay = post(&reads, Some("rewrite-now")).await;
    assert_eq!(replay.status, 202, "{}", replay.text());
    assert_eq!(replay.json(), first.json());
    let calls = reads.header_rewrites.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "a replay never triggers a second run");
    assert_eq!(
        calls[0].report_to, None,
        "the portal reads the Rewrites log"
    );
    assert!(!calls[0].actor.is_empty(), "overrides record who asked");
}

#[tokio::test]
async fn a_trigger_while_a_run_is_active_is_refused() {
    let reads = Reads::new().await;
    *reads.header_rewrite_answer.lock().unwrap() = ManualStart::Running;
    let busy = post(&reads, Some("second")).await;
    assert_eq!(
        (busy.status, busy.api_error()),
        (409, "rewrite_running".into())
    );
    assert_valid(ERROR, "rewrite running", &busy.json());
    // A refusal is not remembered: the same key works once the run ends.
    *reads.header_rewrite_answer.lock().unwrap() = ManualStart::Started(2);
    let later = post(&reads, Some("second")).await;
    assert_eq!(later.status, 202, "{}", later.text());

    *reads.header_rewrite_answer.lock().unwrap() = ManualStart::Disabled;
    let off = post(&reads, None).await;
    assert_eq!((off.status, off.api_error()), (409, "rewrite_off".into()));
    *reads.header_rewrite_answer.lock().unwrap() = ManualStart::Nothing;
    let nothing = post(&reads, None).await;
    assert_eq!(nothing.status, 200, "{}", nothing.text());
    assert_valid(MESSAGE, "nothing to rewrite", &nothing.json());
}

#[tokio::test]
async fn is_unavailable_without_live_delivery() {
    let reads = Reads::without_digest_delivery().await;
    let reply = post(&reads, None).await;
    assert_eq!(
        (reply.status, reply.api_error()),
        (503, "unavailable".into())
    );
    assert_valid(ERROR, "offline header rewrite", &reply.json());
}
