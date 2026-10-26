//! `/api/admin/presence`: two admins (the token session and a Discord admin)
//! at the same Inbox request and run. Admin origin only, CSRF on writes,
//! bounded inputs, never activity for the idle window, a `presence` hint on
//! arrivals and leaves, nothing written, and nobody here after a restart.
//! Expiry and the caps run on paused time in `src/api/presence.rs`.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use chrono::TimeDelta;
use kanade_domain::history::ChangeHistory;
use serde_json::{Value, json};

use crate::{
    events::Stream,
    reads::Reads,
    schemas::assert_valid,
    support::{ADMIN_HOST, Fixture, Reply, get, public, request, send},
};

const PATH: &str = "/api/admin/presence";
const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const ERROR: &str = "error.json#/$defs/ApiError";
const HERE: &str = "presence.json#/$defs/PresenceHere";
const LIST: &str = "presence.json#/$defs/PresenceList";

struct Admins {
    reads: Reads,
    /// The token session (`reads.cookie`) is "Asahi"'s; this is Cara's.
    cara: (String, String),
    /// Each call's `seq`, growing as a page's do.
    seq: AtomicU64,
}

impl Admins {
    async fn new() -> Self {
        let reads = Reads::with_logins().await;
        let cara = reads.discord_session(1003, "Cara").await;
        Self {
            reads,
            cara,
            seq: AtomicU64::new(0),
        }
    }

    fn next(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// The leave path for `at`, with the next `seq`.
    fn leaving(&self, at: &str) -> String {
        format!("{PATH}?at={at}&seq={}", self.next())
    }

    fn asahi(&self) -> (String, String) {
        (self.reads.cookie.clone(), self.reads.csrf.clone())
    }

    async fn send(
        &self,
        session: &(String, String),
        method: &str,
        path: &str,
        body: Option<Value>,
        csrf: bool,
    ) -> Reply {
        let mut headers = vec![("Cookie", session.0.as_str()), ORIGIN];
        if csrf {
            headers.push(("X-Kanade-CSRF", session.1.as_str()));
        }
        let text = body.map(|body| body.to_string());
        send(
            self.reads.admin,
            method,
            ADMIN_HOST,
            path,
            &headers,
            text.as_deref(),
        )
        .await
    }

    /// A check-in; asserts 200 and the schema, answers `here`.
    async fn check_in(
        &self,
        session: &(String, String),
        kind: &str,
        id: &str,
        mode: &str,
    ) -> Value {
        let reply = self
            .send(
                session,
                "PUT",
                PATH,
                Some(json!({"at": {"kind": kind, "id": id}, "mode": mode, "seq": self.next()})),
                true,
            )
            .await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let value = reply.json();
        assert_valid(HERE, PATH, &value);
        assert_eq!(
            (value["heartbeat_s"].clone(), value["ttl_s"].clone()),
            (json!(15), json!(60))
        );
        value["here"].clone()
    }

    async fn read(&self, session: &(String, String), at: &str) -> Value {
        let path = format!("{PATH}?at={at}");
        let reply = request(
            self.reads.admin,
            "GET",
            ADMIN_HOST,
            &path,
            &[("Cookie", session.0.as_str())],
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.text());
        let value = reply.json();
        assert_valid(LIST, &path, &value);
        value["here"].clone()
    }

    async fn refused(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        csrf: bool,
    ) -> (u16, String) {
        let reply = self.send(&self.asahi(), method, path, body, csrf).await;
        assert_valid(ERROR, path, &reply.json());
        (reply.status, reply.api_error())
    }
}

/// `[(name, mode)]` of a `here` list.
fn who(here: &Value) -> Vec<(&str, &str)> {
    here.as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["admin"]["name"].as_str().unwrap(),
                row["mode"].as_str().unwrap(),
            )
        })
        .collect()
}

#[tokio::test]
async fn two_admins_see_each_other_and_never_themselves() {
    let admins = Admins::new().await;
    let asahi = admins.asahi();
    assert_eq!(
        admins
            .check_in(&asahi, "request", "p-kalos", "viewing")
            .await,
        json!([]),
        "alone"
    );
    let here = admins
        .check_in(&admins.cara, "request", "p-kalos", "editing")
        .await;
    assert_eq!(who(&here), [("Break-glass token", "viewing")]);
    assert_eq!(here[0]["admin"]["id"], "token");
    assert!(here[0]["seen_s"].as_u64().unwrap() < 5);

    // Asahi's read (no check-in) shows Cara editing, and not Asahi.
    let here = admins.read(&asahi, "request:p-kalos").await;
    assert_eq!(who(&here), [("Cara", "editing")]);
    assert_eq!(here[0]["admin"]["id"], "discord:1003");
    // A second tab of the same session is the same entry; Asahi's tabs never list Asahi.
    assert_eq!(
        who(&admins
            .check_in(&asahi, "request", "p-kalos", "editing")
            .await),
        [("Cara", "editing")]
    );
    // Another place, another list: the run sheet has nobody yet.
    assert_eq!(admins.read(&asahi, "run:r-kalos").await, json!([]));
    assert_eq!(admins.read(&asahi, "request:p-other").await, json!([]));

    // Cara leaves: gone at once, and a repeated leave is still 204.
    for _ in 0..2 {
        let left = admins
            .send(
                &admins.cara,
                "DELETE",
                &admins.leaving("request:p-kalos"),
                None,
                true,
            )
            .await;
        assert_eq!(left.status, 204, "{}", left.text());
        assert!(left.text().is_empty());
    }
    assert_eq!(admins.read(&asahi, "request:p-kalos").await, json!([]));
}

#[tokio::test]
async fn presence_is_admin_only_and_never_on_the_public_origin() {
    let admins = Admins::new().await;
    for (method, path) in [
        ("GET", format!("{PATH}?at=run:r-kalos")),
        ("PUT", PATH.into()),
        ("DELETE", format!("{PATH}?at=run:r-kalos&seq=1")),
    ] {
        let reply = send(
            admins.reads.admin,
            method,
            ADMIN_HOST,
            &path,
            &[ORIGIN],
            (method == "PUT")
                .then_some(r#"{"at":{"kind":"run","id":"r-kalos"},"mode":"viewing","seq":1}"#),
        )
        .await;
        assert_eq!(reply.error(), (401, "unauthenticated".into()), "{method}");
    }

    let fixture = Fixture::new();
    let http = fixture.http();
    let public = public(&http).await;
    let reply = get(public, "kanade-pub.test", &format!("{PATH}?at=run:r-kalos")).await;
    assert_eq!(reply.error(), (404, "not_found".into()));
}

#[tokio::test]
async fn writes_need_csrf_and_inputs_are_bounded() {
    let admins = Admins::new().await;
    let body = json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": "editing", "seq": 1});
    assert_eq!(
        admins.refused("PUT", PATH, Some(body), false).await,
        (403, "csrf".into())
    );
    assert_eq!(
        admins
            .refused(
                "DELETE",
                &format!("{PATH}?at=run:r-kalos&seq=1"),
                None,
                false
            )
            .await,
        (403, "csrf".into())
    );
    assert_eq!(
        admins.read(&admins.asahi(), "run:r-kalos").await,
        json!([]),
        "nothing checked in"
    );

    let long = "a".repeat(65);
    for body in [
        json!({"at": {"kind": "draft", "id": "d-weekend"}, "mode": "viewing", "seq": 1}),
        json!({"at": {"kind": "run", "id": ""}, "mode": "viewing", "seq": 1}),
        json!({"at": {"kind": "run", "id": long}, "mode": "viewing", "seq": 1}),
        json!({"at": {"kind": "run", "id": "../x"}, "mode": "viewing", "seq": 1}),
        json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": "watching", "seq": 1}),
        json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": "viewing", "seq": 1, "extra": 1}),
        json!({"at": {"kind": "run", "id": "r-kalos"}, "seq": 1}),
        json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": "viewing"}),
        json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": "viewing", "seq": -1}),
    ] {
        assert_eq!(
            admins.refused("PUT", PATH, Some(body.clone()), true).await,
            (400, "invalid_body".into()),
            "{body}"
        );
    }
    for query in [
        "",
        "?at=draft:d-weekend",
        "?at=run:",
        "?at=run:r-kalos&at=run:r-x",
        "?at=run:r-kalos&x=1",
        "?id=r-kalos",
    ] {
        for method in ["GET", "DELETE"] {
            assert_eq!(
                admins
                    .refused(method, &format!("{PATH}{query}"), None, true)
                    .await,
                (422, "invalid_query".into()),
                "{method} {query}"
            );
        }
    }
    // The leave needs its seq; the read takes none.
    for query in [
        "?at=run:r-kalos",
        "?at=run:r-kalos&seq=",
        "?at=run:r-kalos&seq=x",
        "?at=run:r-kalos&seq=1&seq=2",
    ] {
        assert_eq!(
            admins
                .refused("DELETE", &format!("{PATH}{query}"), None, true)
                .await,
            (422, "invalid_query".into()),
            "DELETE {query}"
        );
    }
    assert_eq!(
        admins
            .refused("GET", &format!("{PATH}?at=run:r-kalos&seq=1"), None, true)
            .await,
        (422, "invalid_query".into())
    );
}

#[tokio::test]
async fn a_check_in_that_lands_after_its_leave_does_not_bring_the_tab_back() {
    let admins = Admins::new().await;
    let put = |seq: u64, mode: &str| json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": mode, "seq": seq});
    let ok = |reply: Reply| assert!((200..300).contains(&reply.status), "{}", reply.text());
    ok(admins
        .send(&admins.cara, "PUT", PATH, Some(put(2, "editing")), true)
        .await);
    // Cara's page closes: its check-in (seq 3) is cut off in flight, the leave (seq 3) lands first.
    ok(admins
        .send(
            &admins.cara,
            "DELETE",
            &format!("{PATH}?at=run:r-kalos&seq=3"),
            None,
            true,
        )
        .await);
    ok(admins
        .send(&admins.cara, "PUT", PATH, Some(put(3, "editing")), true)
        .await);
    assert_eq!(
        admins.read(&admins.asahi(), "run:r-kalos").await,
        json!([]),
        "the late check-in is ignored"
    );
    // Opened again: the next check-in (seq 4) is back.
    ok(admins
        .send(&admins.cara, "PUT", PATH, Some(put(4, "viewing")), true)
        .await);
    assert_eq!(
        who(&admins.read(&admins.asahi(), "run:r-kalos").await),
        [("Cara", "viewing")]
    );
}

#[tokio::test]
async fn checking_in_never_extends_the_idle_window() {
    let admins = Admins::new().await;
    let asahi = admins.asahi();
    *admins.reads.session_skew.lock().unwrap() = TimeDelta::minutes(59);
    admins.check_in(&asahi, "run", "r-kalos", "editing").await;
    admins.read(&asahi, "run:r-kalos").await;
    let left = admins
        .send(&asahi, "DELETE", &admins.leaving("run:r-kalos"), None, true)
        .await;
    assert_eq!(left.status, 204);
    // Past the idle limit counted from the last real request: the session is over.
    *admins.reads.session_skew.lock().unwrap() = TimeDelta::minutes(61);
    let reply = admins
        .send(
            &asahi,
            "PUT",
            PATH,
            Some(json!({"at": {"kind": "run", "id": "r-kalos"}, "mode": "viewing", "seq": admins.next()})),
            true,
        )
        .await;
    assert_eq!(reply.status, 401, "check-ins did not count as activity");
    let reply = request(
        admins.reads.admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/session",
        &[("Cookie", asahi.0.as_str())],
    )
    .await;
    assert_eq!(reply.status, 401);
}

#[tokio::test]
async fn arrivals_mode_changes_and_leaves_hint_presence_and_heartbeats_do_not() {
    let admins = Admins::new().await;
    let mut stream = Stream::open(admins.reads.admin, &admins.reads.cookie).await;
    stream.ready().await;
    let head = admins.reads.store.history_head().await.unwrap().seq;

    admins
        .check_in(&admins.cara, "run", "r-kalos", "viewing")
        .await;
    assert_eq!(stream.hint().await["topic"], "presence");
    admins
        .check_in(&admins.cara, "run", "r-kalos", "viewing")
        .await;
    admins
        .check_in(&admins.cara, "run", "r-kalos", "editing")
        .await;
    let left = admins
        .send(
            &admins.cara,
            "DELETE",
            &admins.leaving("run:r-kalos"),
            None,
            true,
        )
        .await;
    assert_eq!(left.status, 204);
    assert_eq!(
        stream.topics(Duration::from_millis(500)).await,
        ["presence", "presence"],
        "the mode change and the leave; the repeated heartbeat hinted nothing"
    );
    assert_eq!(
        admins.reads.store.history_head().await.unwrap().seq,
        head,
        "presence writes no History record"
    );
}

#[tokio::test]
async fn a_restarted_process_starts_with_nobody_here() {
    let mut admins = Admins::new().await;
    admins
        .check_in(&admins.cara, "request", "p-kalos", "editing")
        .await;
    assert_eq!(
        who(&admins.read(&admins.asahi(), "request:p-kalos").await).len(),
        1
    );
    admins.reads.restart(TimeDelta::zero(), None).await;
    assert_eq!(
        admins.read(&admins.asahi(), "request:p-kalos").await,
        json!([])
    );
}
