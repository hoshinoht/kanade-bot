//! Admin drafts against the seeded store of `reads.rs` (pinned clock Tue 29
//! Sep 12:00 KL), with two admins: the token session drafts, a Discord
//! admin changes the live schedule underneath it. Every answer is validated
//! against the contract schemas.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::{
    history::{Actor, ChangeHistory, Origin, Surface},
    notify::NoticeOutbox,
    scheduler::{ScheduleStore, Scope},
};
use serde_json::{Value, json};

use crate::{
    reads::{Reads, policy, scheduler, utc},
    schemas::assert_valid,
    support::{ADMIN_HOST, Reply, fields, find, request, send, strs},
};

const ORIGIN: (&str, &str) = ("Origin", "https://kanade.test");
const ERROR: &str = "error.json#/$defs/ApiError";

/// The other admin moves a run on the live schedule.
async fn moved_upstream(store: &Arc<SqliteStore>, run: &str, to: DateTime<Utc>) {
    moved_upstream_at(store, run, to, utc(9, 29, 4, 0)).await;
}

/// As [`moved_upstream`], recorded at `at`.
async fn moved_upstream_at(
    store: &Arc<SqliteStore>,
    run: &str,
    to: DateTime<Utc>,
    at: DateTime<Utc>,
) {
    scheduler(store, at)
        .as_origin(Origin::new(
            Actor::admin("discord:1003"),
            Surface::AdminPortal,
        ))
        .amend_run(run, to, &policy())
        .await
        .unwrap();
}

struct Desk {
    reads: Reads,
    other: (String, String),
}

impl Desk {
    async fn new() -> Self {
        let reads = Reads::with_logins().await;
        let other = reads.discord_session(1003, "Cara").await;
        Self { reads, other }
    }

    fn token(&self) -> (String, String) {
        (self.reads.cookie.clone(), self.reads.csrf.clone())
    }

    async fn call(
        &self,
        session: &(String, String),
        method: &str,
        path: &str,
        body: Option<Value>,
        extra: &[(&str, &str)],
    ) -> Reply {
        let mut headers = vec![
            ("Cookie", session.0.as_str()),
            ORIGIN,
            ("X-Kanade-CSRF", session.1.as_str()),
        ];
        headers.extend_from_slice(extra);
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

    async fn write(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        key: Option<&str>,
    ) -> Reply {
        let extra: Vec<(&str, &str)> = key
            .map(|key| ("Idempotency-Key", key))
            .into_iter()
            .collect();
        self.call(&self.token(), method, path, body, &extra).await
    }

    async fn get(&self, session: &(String, String), path: &str, schema: &str) -> (u16, Value) {
        let reply = request(
            self.reads.admin,
            "GET",
            ADMIN_HOST,
            path,
            &[("Cookie", session.0.as_str())],
        )
        .await;
        let value = reply.json();
        if reply.status == 200 {
            assert_valid(schema, path, &value);
        }
        (reply.status, value)
    }

    async fn draft(&self, id: &str) -> Value {
        let (status, value) = self
            .get(
                &self.token(),
                &format!("/api/admin/drafts/{id}"),
                "drafts.json#/$defs/DraftDetail",
            )
            .await;
        assert_eq!(status, 200, "{value}");
        value
    }

    async fn preview(&self, id: &str) -> Value {
        let path = format!("/api/admin/drafts/{id}/preview");
        let (status, value) = self
            .get(&self.token(), &path, "drafts.json#/$defs/DraftPreview")
            .await;
        assert_eq!(status, 200, "{value}");
        value
    }

    async fn head(&self) -> u64 {
        self.reads.store.history_head().await.unwrap().seq
    }

    async fn outbox(&self) -> usize {
        self.reads.store.outbox_notices().await.unwrap().len()
    }
}

/// A 2xx draft write's answer, validated.
fn written(reply: &Reply) -> Value {
    assert!((200..300).contains(&reply.status), "{}", reply.text());
    let value = reply.json();
    assert_valid("drafts.json#/$defs/DraftWritten", "draft write", &value);
    value
}

fn refused(reply: &Reply) -> (u16, String) {
    assert!(reply.status >= 400, "{}", reply.text());
    assert_valid(ERROR, "draft refusal", &reply.json());
    (reply.status, reply.api_error())
}

fn code(status: u16, code: &str) -> (u16, String) {
    (status, code.to_owned())
}

async fn create(desk: &Desk, title: &str) -> String {
    let reply = desk
        .write(
            "POST",
            "/api/admin/drafts",
            Some(json!({"title": title})),
            None,
        )
        .await;
    assert_eq!(reply.status, 201, "{}", reply.text());
    written(&reply)["id"].as_str().unwrap().to_owned()
}

async fn stage(desk: &Desk, id: &str, version: u64, op: Value) -> u64 {
    let reply = desk
        .write(
            "POST",
            &format!("/api/admin/drafts/{id}/ops"),
            Some(json!({"version": version, "op": op})),
            None,
        )
        .await;
    written(&reply)["version"].as_u64().unwrap()
}

/// Stale base → merge refused → rebase keeps the edit → merge; replay,
/// CSRF, versions and ownership on the way. Nothing reaches the schedule,
/// History or the notice outbox before the merge.
#[tokio::test]
async fn a_draft_on_a_moved_base_rebases_keeping_its_edit_then_merges() {
    let desk = Desk::new().await;
    let head = desk.head().await;
    let outbox = desk.outbox().await;

    // Create: keyed, replayed, and a reused key for another title refused.
    let key = Some("draft-create-1");
    let body = json!({"title": "Weekend reshuffle"});
    let first = desk
        .write("POST", "/api/admin/drafts", Some(body.clone()), key)
        .await;
    assert_eq!(first.status, 201, "{}", first.text());
    let created = written(&first);
    let again = desk
        .write("POST", "/api/admin/drafts", Some(body), key)
        .await;
    assert_eq!((again.status, written(&again)), (201, created.clone()));
    assert_eq!(
        refused(
            &desk
                .write(
                    "POST",
                    "/api/admin/drafts",
                    Some(json!({"title": "Other"})),
                    key
                )
                .await
        ),
        code(422, "idempotency_mismatch")
    );
    let id = created["id"].as_str().unwrap().to_owned();

    // CSRF is required on every write.
    let body = json!({"version": 1, "op": {"kind": "reset", "run": "r-kalos"}});
    let ops = format!("/api/admin/drafts/{id}/ops");
    let no_csrf = desk.reads.without_csrf("POST", &ops, &body).await;
    assert_eq!(no_csrf.error(), (403, "csrf".into()));

    // Stage a move of r-kalos to Wed 30 Sep 21:30 (day 6 of the boss week).
    let op = json!({"kind": "move", "run": "r-kalos", "day": 6, "time": "21:30"});
    for (body, expected) in [
        (json!({"op": op}), code(422, "version_required")),
        (json!({"version": 9, "op": op}), code(409, "stale")),
        (
            json!({"version": 1, "op": {"kind": "move", "run": "r-kalos", "day": 7, "time": "21:30"}}),
            code(422, "invalid"),
        ),
    ] {
        assert_eq!(
            refused(&desk.write("POST", &ops, Some(body.clone()), None).await),
            expected,
            "{body}"
        );
    }
    let staged = desk
        .write(
            "POST",
            &ops,
            Some(json!({"version": 1, "op": op})),
            Some("stage-1"),
        )
        .await;
    let version = written(&staged)["version"].as_u64().unwrap();
    assert_eq!(version, 2);
    let replay = desk
        .write(
            "POST",
            &ops,
            Some(json!({"version": 1, "op": op})),
            Some("stage-1"),
        )
        .await;
    assert_eq!(written(&replay), written(&staged), "a keyed retry replays");
    assert_eq!(desk.draft(&id).await["staged"], json!(1));

    // Only the author sees it.
    let (status, _) = desk
        .get(&desk.other, &format!("/api/admin/drafts/{id}"), "")
        .await;
    assert_eq!(status, 404);
    let (_, list) = desk
        .get(
            &desk.other,
            "/api/admin/drafts",
            "drafts.json#/$defs/DraftList",
        )
        .await;
    assert_eq!(list["drafts"], json!([]));

    // A rename is versioned too.
    let path = format!("/api/admin/drafts/{id}");
    let renamed = desk
        .write(
            "PATCH",
            &path,
            Some(json!({"version": 2, "title": "Wednesday Kalos"})),
            None,
        )
        .await;
    assert_eq!(written(&renamed)["version"], json!(3));
    assert_eq!(desk.draft(&id).await["title"], json!("Wednesday Kalos"));

    // Nothing live yet.
    assert_eq!(desk.head().await, head);
    assert_eq!(desk.outbox().await, outbox);
    let preview = desk.preview(&id).await;
    assert_eq!(
        (&preview["up_to_date"], &preview["mergeable"]),
        (&json!(true), &json!(true))
    );
    assert!(!preview["rows"].as_array().unwrap().is_empty());

    // The other admin moves r-kalos on the live schedule.
    moved_upstream(&desk.reads.store, "r-kalos", utc(9, 30, 12, 0)).await;
    let moved_at = desk.head().await;
    let preview = desk.preview(&id).await;
    assert_eq!(
        (&preview["up_to_date"], &preview["mergeable"]),
        (&json!(false), &json!(false))
    );
    let conflict = &preview["conflicts"][0];
    assert_eq!(conflict["kind"], json!("both_changed"));
    assert_eq!(conflict["field"], json!("slot"));
    assert_eq!(conflict["ops"], json!([0]));
    assert_eq!(conflict["settle"], json!(["keep", "take"]));
    assert_eq!(conflict["record"]["seq"], json!(moved_at));
    assert_eq!(conflict["record"]["actor"]["id"], json!("discord:1003"));
    let (_, list) = desk
        .get(
            &desk.token(),
            "/api/admin/drafts",
            "drafts.json#/$defs/DraftList",
        )
        .await;
    assert_eq!(list["drafts"][0]["up_to_date"], json!(false));

    // Merge needs the head; a rebase needs every conflict settled.
    let merge = format!("/api/admin/drafts/{id}/merge");
    let rebase = format!("/api/admin/drafts/{id}/rebase");
    assert_eq!(
        refused(
            &desk
                .write("POST", &merge, Some(json!({"version": 3})), None)
                .await
        ),
        code(409, "base_moved")
    );
    assert_eq!(
        refused(
            &desk
                .write("POST", &rebase, Some(json!({"version": 3})), None)
                .await
        ),
        code(409, "conflicts")
    );
    assert_eq!(
        refused(
            &desk
                .write(
                    "POST",
                    &rebase,
                    Some(json!({"version": 3, "settle": {"x": "maybe"}})),
                    None
                )
                .await
        ),
        code(422, "invalid")
    );
    let key = conflict["key"].as_str().unwrap();
    let rebased = desk
        .write(
            "POST",
            &rebase,
            Some(json!({"version": 3, "settle": {key: "keep"}})),
            Some("rebase-1"),
        )
        .await;
    assert_eq!(written(&rebased)["version"], json!(4));
    assert_eq!(
        desk.draft(&id).await["staged"],
        json!(1),
        "the edit is kept"
    );
    assert_eq!(desk.head().await, moved_at, "a rebase is not live");
    let preview = desk.preview(&id).await;
    assert_eq!(
        (&preview["up_to_date"], &preview["mergeable"]),
        (&json!(true), &json!(true))
    );

    // Merge: one History record and its notices, then the draft is closed.
    let outbox = desk.outbox().await;
    let merged = desk
        .write("POST", &merge, Some(json!({"version": 4})), Some("merge-1"))
        .await;
    let answer = written(&merged);
    assert_eq!(answer["status"], json!("merged"));
    assert_eq!(answer["seq"], json!(moved_at + 1));
    assert_eq!(
        written(
            &desk
                .write("POST", &merge, Some(json!({"version": 4})), Some("merge-1"))
                .await
        ),
        answer,
        "a keyed retry replays the merge"
    );
    assert_eq!(desk.head().await, moved_at + 1);
    assert!(desk.outbox().await > outbox, "the merge writes its notices");
    let record = desk
        .reads
        .store
        .load_change(moved_at + 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.surface, Surface::DraftMerge);
    assert_eq!(record.origin.actor, Actor::admin("token"));
    let run = desk
        .reads
        .store
        .load(&Scope::All)
        .await
        .unwrap()
        .runs
        .into_iter()
        .find(|run| run.id == "r-kalos")
        .unwrap();
    assert_eq!(run.datetime, utc(9, 30, 13, 30), "the draft's time won");
    assert_eq!(
        refused(
            &desk
                .write("POST", &ops, Some(json!({"version": 4, "op": op})), None)
                .await
        ),
        code(409, "stale"),
        "a merged draft takes no more changes"
    );
}

/// Take theirs drops the operation; discard closes without applying; the
/// closed list shows merged and discarded drafts, the open list neither.
#[tokio::test]
async fn take_drops_the_operation_and_closed_drafts_are_listed_read_only() {
    let desk = Desk::new().await;
    let head = desk.head().await;
    let taken = create(&desk, "Next week").await;
    let version = stage(
        &desk,
        &taken,
        1,
        json!({"kind": "move", "run": "n-kalos", "day": 5, "time": "21:00"}),
    )
    .await;
    moved_upstream(&desk.reads.store, "n-kalos", utc(10, 6, 15, 0)).await;
    let preview = desk.preview(&taken).await;
    let key = preview["conflicts"][0]["key"]
        .as_str()
        .unwrap_or_else(|| panic!("a conflict: {preview}"))
        .to_owned();
    let rebased = desk
        .write(
            "POST",
            &format!("/api/admin/drafts/{taken}/rebase"),
            Some(json!({"version": version, "settle": {key: "take"}})),
            None,
        )
        .await;
    let version = written(&rebased)["version"].as_u64().unwrap();
    assert_eq!(desk.draft(&taken).await["staged"], json!(0), "theirs taken");
    assert_eq!(
        refused(
            &desk
                .write(
                    "POST",
                    &format!("/api/admin/drafts/{taken}/merge"),
                    Some(json!({"version": version})),
                    None,
                )
                .await
        ),
        code(422, "empty")
    );

    // Remove an operation, then discard.
    let dropped = create(&desk, "Scratch").await;
    let version = stage(
        &desk,
        &dropped,
        1,
        json!({"kind": "status", "run": "n-star", "status": "cancelled"}),
    )
    .await;
    let removed = desk
        .write(
            "DELETE",
            &format!("/api/admin/drafts/{dropped}/ops/0?version={version}"),
            None,
            None,
        )
        .await;
    let version = written(&removed)["version"].as_u64().unwrap();
    assert_eq!(desk.draft(&dropped).await["staged"], json!(0));
    let path = format!("/api/admin/drafts/{dropped}");
    assert_eq!(
        refused(&desk.write("DELETE", &path, None, None).await),
        code(422, "version_required")
    );
    let discarded = desk
        .write(
            "DELETE",
            &format!("{path}?version={version}"),
            None,
            Some("discard-1"),
        )
        .await;
    assert_eq!(written(&discarded)["status"], json!("discarded"));
    assert_eq!(
        refused(
            &desk
                .write(
                    "PATCH",
                    &path,
                    Some(json!({"version": version, "title": "Again"})),
                    None
                )
                .await
        ),
        code(409, "stale")
    );

    let (_, open) = desk
        .get(
            &desk.token(),
            "/api/admin/drafts?state=open",
            "drafts.json#/$defs/DraftList",
        )
        .await;
    assert_eq!(ids(&open), std::slice::from_ref(&taken));
    let (_, closed) = desk
        .get(
            &desk.token(),
            "/api/admin/drafts?state=closed",
            "drafts.json#/$defs/DraftList",
        )
        .await;
    assert_eq!(ids(&closed), std::slice::from_ref(&dropped));
    assert_eq!(closed["drafts"][0]["status"], json!("discarded"));
    let (status, _) = desk
        .get(&desk.token(), "/api/admin/drafts?state=other", "")
        .await;
    assert_eq!(status, 422);
    // One History record: the other admin's move. Drafts wrote none.
    assert_eq!(desk.head().await, head + 1);
}

/// The History graph's lanes: own drafts only, any status, each listed while
/// its span (base to merge, to the last record before its close, or to the
/// head) touches the asked records; closed drafts carry their close time.
#[tokio::test]
async fn the_graph_lists_own_drafts_spanning_the_asked_records() {
    let desk = Desk::new().await;
    let base = desk.head().await;
    let open = create(&desk, "Weekend reshuffle").await;
    let scratch = create(&desk, "Scratch").await;
    let discarded = desk
        .write(
            "DELETE",
            &format!("/api/admin/drafts/{scratch}?version=1"),
            None,
            None,
        )
        .await;
    assert_eq!(written(&discarded)["status"], json!("discarded"));
    let merged = create(&desk, "Tuesday Kalos").await;
    let version = stage(
        &desk,
        &merged,
        1,
        json!({"kind": "move", "run": "r-kalos", "day": 6, "time": "21:30"}),
    )
    .await;
    let answer = desk
        .write(
            "POST",
            &format!("/api/admin/drafts/{merged}/merge"),
            Some(json!({"version": version})),
            None,
        )
        .await;
    let merge_seq = written(&answer)["seq"].as_u64().unwrap();
    assert_eq!(merge_seq, base + 1);
    // Another admin's draft never shows.
    let theirs = desk
        .call(
            &desk.other,
            "POST",
            "/api/admin/drafts",
            Some(json!({"title": "Not mine"})),
            &[],
        )
        .await;
    assert_eq!(theirs.status, 201, "{}", theirs.text());
    // A later record, an hour after the discard and the merge.
    moved_upstream_at(
        &desk.reads.store,
        "r-star",
        utc(9, 30, 14, 0),
        utc(9, 29, 5, 0),
    )
    .await;
    let later = desk.head().await;
    assert_eq!(later, merge_seq + 1);

    let graph = |from: u64, to: u64| {
        let desk = &desk;
        async move {
            let path = format!("/api/admin/drafts/graph?from={from}&to={to}");
            let (status, value) = desk
                .get(&desk.token(), &path, "drafts.json#/$defs/DraftGraph")
                .await;
            assert_eq!(status, 200, "{value}");
            value
        }
    };
    let all = graph(base, later).await;
    assert_eq!(all["head"], json!(later));
    let mut every = vec![open.clone(), scratch.clone(), merged.clone()];
    every.sort();
    assert_eq!(ids(&all), every);
    let row = |value: &Value, id: &str| find(&value["drafts"], "id", id).clone();
    let open_row = row(&all, &open);
    assert_eq!(
        fields(&open_row, &["status", "closed_at", "up_to_date"]),
        json!(["open", null, false]),
        "base moved, no conflict count"
    );
    for (id, status) in [(&scratch, "discarded"), (&merged, "merged")] {
        let closed = row(&all, id);
        assert_eq!(closed["status"], json!(status));
        assert_eq!(
            closed["closed_at"], closed["updated_at"],
            "{id}: the close is the row's last change"
        );
    }
    assert_eq!(row(&all, &merged)["merged_seq"], json!(merge_seq));

    // After the merge and the discard: only the open draft still spans.
    assert_eq!(ids(&graph(later, later).await), std::slice::from_ref(&open));
    // Below every base: nothing.
    if base > 0 {
        assert!(ids(&graph(0, base - 1).await).is_empty());
    }
    // The merge record itself: the merged draft and the open one.
    assert!(ids(&graph(merge_seq, merge_seq).await).contains(&merged));

    for query in [
        "",
        "?from=1",
        "?to=1",
        "?from=2&to=1",
        "?from=a&to=1",
        "?from=&to=1",
        "?from=1&to=1&from=1",
        "?from=1&to=1&state=open",
        "?from=1&to=99999999999999999999",
    ] {
        let (status, value) = desk
            .get(
                &desk.token(),
                &format!("/api/admin/drafts/graph{query}"),
                "",
            )
            .await;
        assert_eq!(status, 422, "{query}: {value}");
        assert_eq!(value["error"], json!("invalid_query"), "{query}");
    }
    // Signed out: refused like every admin read.
    let anonymous = request(
        desk.reads.admin,
        "GET",
        ADMIN_HOST,
        &format!("/api/admin/drafts/graph?from={base}&to={later}"),
        &[],
    )
    .await;
    assert_eq!(anonymous.status, 401);
    // The other admin sees only their own draft.
    let (_, other) = desk
        .get(
            &desk.other,
            &format!("/api/admin/drafts/graph?from={base}&to={later}"),
            "drafts.json#/$defs/DraftGraph",
        )
        .await;
    assert_eq!(other["drafts"].as_array().unwrap().len(), 1);
    assert_eq!(other["drafts"][0]["title"], json!("Not mine"));
}

/// A draft list's ids, sorted.
fn ids(list: &Value) -> Vec<String> {
    let mut ids: Vec<String> = strs(&list["drafts"], "id")
        .into_iter()
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids
}
