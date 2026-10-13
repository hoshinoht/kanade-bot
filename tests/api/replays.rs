//! `Idempotency-Key` replays that survive a restart: the Limits window clear,
//! manual digest and header rewrite, rescan submit and cancel, and the admin
//! member edits. After [`Reads::restart`] (fresh desks, no in-memory state,
//! same store file) a key replays its stored answer without reapplying,
//! another request under it is refused, an expired key applies anew, and
//! expired rows are pruned.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use kanade::domain::settings::{SettingsChangeQuery, SettingsStore};
use kanade::infrastructure::store::replays::{REPLAY_TTL, ReplayScope, ReplayStore};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, Connection, sqlite::SqliteConnectOptions};

use crate::{
    reads::Reads,
    support::{ADMIN_HOST, Reply, request},
};

const RESET: &str = "/api/admin/limits/windows/1001";
const DIGEST: &str = "/api/admin/digest";
const HEADERS: &str = "/api/admin/headers/rewrite";
const RESCAN: &str = "/api/admin/rescan";

/// The pinned clock of [`Reads`].
fn pinned() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap()
}

/// Past a key's lifetime.
fn expired() -> TimeDelta {
    REPLAY_TTL + TimeDelta::hours(1)
}

async fn keyed(reads: &Reads, method: &str, path: &str, key: &str, body: Value) -> Reply {
    reads
        .call(method, path, body, &[("Idempotency-Key", key)])
        .await
}

async fn limit_records(reads: &Reads) -> usize {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .iter()
        .filter(|change| change.section == "limits")
        .count()
}

/// Run SQL against the live store file beside its owner (tests only).
async fn raw(reads: &Reads, sql: &str) {
    let mut conn = SqliteConnectOptions::new()
        .filename(&reads.db_path)
        .connect()
        .await
        .expect("raw connection");
    sqlx::raw_sql(sql)
        .execute(&mut conn)
        .await
        .expect("raw sql");
    conn.close().await.expect("close");
}

#[tokio::test]
async fn a_window_clear_replays_across_a_restart_and_expires() {
    let mut reads = Reads::new().await;
    reads.chat.spend("1001");
    let first = keyed(&reads, "DELETE", RESET, "clear-r", json!({})).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!(limit_records(&reads).await, 1);

    reads.restart(TimeDelta::zero(), None).await;
    reads.chat.spend("1001");
    let replay = keyed(&reads, "DELETE", RESET, "clear-r", json!({})).await;
    assert_eq!((replay.status, replay.json()), (200, first.json()));
    assert_eq!(
        reads.chat.resets.lock().unwrap().len(),
        1,
        "the replay cleared nothing"
    );
    assert_eq!(limit_records(&reads).await, 1, "and recorded nothing");
    let other = keyed(
        &reads,
        "DELETE",
        "/api/admin/limits/windows/1002",
        "clear-r",
        json!({}),
    )
    .await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));
    // The digest and header rewrite share the Limits desk's keys.
    let digest = keyed(
        &reads,
        "POST",
        DIGEST,
        "clear-r",
        json!({"week": "this", "channel_id": "kalos-four"}),
    )
    .await;
    assert_eq!(digest.error(), (422, "idempotency_mismatch".into()));

    reads.restart(expired(), None).await;
    let anew = keyed(&reads, "DELETE", RESET, "clear-r", json!({})).await;
    assert_eq!(anew.status, 200, "{}", anew.text());
    assert_eq!(reads.chat.resets.lock().unwrap().len(), 2, "applied anew");
    assert_eq!(
        limit_records(&reads).await,
        2,
        "the second answer is recorded"
    );
}

/// The volatile allowance is reset before History is written: a History
/// write that fails leaves the window cleared and records nothing, and the
/// retry finds it empty and records nothing either.
#[tokio::test]
async fn a_clear_records_history_only_after_the_reset() {
    let reads = Reads::new().await;
    reads.chat.spend("1001");
    reads.chat.spend("1001");
    raw(
        &reads,
        "CREATE TRIGGER refuse_history BEFORE INSERT ON settings_changes \
         BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    )
    .await;
    let failed = keyed(&reads, "DELETE", RESET, "clear-f", json!({})).await;
    assert_eq!(failed.status, 503, "{}", failed.text());
    assert_eq!(reads.chat.resets.lock().unwrap().as_slice(), ["1001"]);
    assert_eq!(limit_records(&reads).await, 0);
    assert_eq!(
        reads
            .store
            .replay(ReplayScope::Limits, "admin:token", "clear-f", pinned())
            .await
            .unwrap(),
        None,
        "a refused record stores no replay"
    );
    raw(&reads, "DROP TRIGGER refuse_history;").await;

    let retry = keyed(&reads, "DELETE", RESET, "clear-f", json!({})).await;
    assert_eq!(retry.status, 200, "{}", retry.text());
    assert_eq!(
        limit_records(&reads).await,
        0,
        "an empty window records nothing"
    );
    let replay = keyed(&reads, "DELETE", RESET, "clear-f", json!({})).await;
    assert_eq!(replay.json(), retry.json());
    assert_eq!(
        reads.chat.resets.lock().unwrap().len(),
        2,
        "the retry reset, its replay did not"
    );
}

#[tokio::test]
async fn a_digest_and_a_header_rewrite_replay_across_a_restart_and_expire() {
    let mut reads = Reads::new().await;
    let body = json!({"week": "this", "channel_id": "kalos-four"});
    let digest = keyed(&reads, "POST", DIGEST, "digest-r", body.clone()).await;
    assert_eq!(digest.status, 200, "{}", digest.text());
    let rewrite = keyed(&reads, "POST", HEADERS, "rewrite-r", json!({})).await;
    assert_eq!(rewrite.status, 202, "{}", rewrite.text());

    reads.restart(TimeDelta::zero(), None).await;
    let replayed = keyed(&reads, "POST", DIGEST, "digest-r", body.clone()).await;
    assert_eq!((replayed.status, replayed.json()), (200, digest.json()));
    let again = keyed(&reads, "POST", HEADERS, "rewrite-r", json!({})).await;
    assert_eq!((again.status, again.json()), (202, rewrite.json()));
    assert_eq!(reads.digest_posts.lock().unwrap().len(), 1, "posted once");
    assert_eq!(
        reads.header_rewrites.lock().unwrap().len(),
        1,
        "queued once"
    );
    // `channel_id` left out is another request than naming the channel.
    let default_channel = keyed(&reads, "POST", DIGEST, "digest-r", json!({"week": "this"})).await;
    assert_eq!(
        default_channel.error(),
        (422, "idempotency_mismatch".into())
    );

    reads.restart(expired(), None).await;
    let anew = keyed(&reads, "POST", DIGEST, "digest-r", body).await;
    assert_eq!(anew.status, 200, "{}", anew.text());
    assert_eq!(reads.digest_posts.lock().unwrap().len(), 2, "applied anew");
}

#[tokio::test]
async fn rescan_submit_and_cancel_replay_across_a_restart() {
    let mut reads = Reads::new().await;
    let body = json!({"channels": ["kalos-four", "limbo-trio"], "window": "week"});
    let submitted = keyed(&reads, "POST", RESCAN, "scan-r", body.clone()).await;
    assert_eq!(submitted.status, 200, "{}", submitted.text());
    let id = submitted.json()["id"].as_str().unwrap().to_owned();
    let path = format!("{RESCAN}/{id}");
    let cancelled = keyed(&reads, "DELETE", &path, "stop-r", json!({})).await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.text());

    reads.restart(TimeDelta::zero(), None).await;
    let replay = keyed(&reads, "POST", RESCAN, "scan-r", body.clone()).await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["id"], id.as_str(), "the recorded job");
    assert_eq!(replay.json()["state"], "cancelled", "in its current state");
    assert_eq!(
        reads.rescans.requests.lock().unwrap().len(),
        1,
        "submitted once"
    );
    let stop = keyed(&reads, "DELETE", &path, "stop-r", json!({})).await;
    assert_eq!((stop.status, stop.json()["id"].clone()), (200, json!(id)));
    let other = keyed(
        &reads,
        "POST",
        RESCAN,
        "scan-r",
        json!({"channels": ["kalos-four"], "window": "week"}),
    )
    .await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));

    reads.restart(expired(), None).await;
    let anew = keyed(&reads, "POST", RESCAN, "scan-r", body).await;
    assert_eq!(anew.status, 200, "{}", anew.text());
    assert_ne!(anew.json()["id"], id.as_str(), "a new job");
    assert_eq!(reads.rescans.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn expired_replays_are_pruned() {
    let reads = Reads::new().await;
    for key in ["p-1", "p-2"] {
        let reply = keyed(
            &reads,
            "POST",
            DIGEST,
            key,
            json!({"week": "this", "channel_id": "kalos-four"}),
        )
        .await;
        assert_eq!(reply.status, 200, "{}", reply.text());
    }
    assert_eq!(reads.store.prune_replays(pinned()).await.unwrap(), 0);
    assert_eq!(
        reads
            .store
            .prune_replays(pinned() + REPLAY_TTL)
            .await
            .unwrap(),
        2
    );
}

async fn member_records(reads: &Reads) -> Vec<kanade::domain::settings::SettingsChange> {
    reads
        .store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|change| change.section == "members")
        .collect()
}

/// Member patch and alias writes record History rows (beside the Config
/// saves) and replay their key without a second write, as the other admin
/// writes do.
#[tokio::test]
async fn member_edits_are_recorded_and_replay_across_a_restart() {
    use kanade::domain::members::{MemberStore, PingLevel};

    let mut reads = Reads::new().await;
    let alice = "/api/admin/members/1001";
    let first = keyed(
        &reads,
        "PATCH",
        alice,
        "member-k",
        json!({"ping_level": "off"}),
    )
    .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let records = member_records(&reads).await;
    assert_eq!(records.len(), 1, "one History row");
    let row = &records[0].values["member.1001"];
    assert!(row.from.contains(r#""ping_level":"essential""#), "{row:?}");
    assert!(row.to.contains(r#""ping_level":"off""#), "{row:?}");
    assert_eq!(records[0].actor.kind(), "admin");

    reads.restart(TimeDelta::zero(), None).await;
    // Behind the key, the row moves on; the replay must not undo that.
    reads
        .store
        .apply_portal(
            "1001",
            kanade::domain::members::PortalEdit {
                ping_level: Some(PingLevel::All),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let replay = keyed(
        &reads,
        "PATCH",
        alice,
        "member-k",
        json!({"ping_level": "off"}),
    )
    .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(
        replay.json()["ping_level"],
        "all",
        "the current row, not a second write"
    );
    assert_eq!(
        member_records(&reads).await.len(),
        1,
        "and nothing recorded"
    );
    let other = keyed(
        &reads,
        "PATCH",
        alice,
        "member-k",
        json!({"ping_level": "essential"}),
    )
    .await;
    assert_eq!(other.error(), (422, "idempotency_mismatch".into()));
    let stored = reads.store.load_member("1001").await.unwrap().unwrap();
    assert_eq!(
        stored.member.ping_level,
        PingLevel::All,
        "the mismatch wrote nothing"
    );

    let bob = "/api/admin/members/1002/aliases";
    let added = keyed(&reads, "POST", bob, "alias-k", json!({"alias": "bobby"})).await;
    assert_eq!(added.status, 200, "{}", added.text());
    let again = keyed(&reads, "POST", bob, "alias-k", json!({"alias": "bobby"})).await;
    assert_eq!((again.status, again.json()), (200, added.json()));
    assert_eq!(member_records(&reads).await.len(), 2, "the alias once");
    let gone = keyed(
        &reads,
        "DELETE",
        &format!("{bob}/bobby"),
        "alias-r",
        json!({}),
    )
    .await;
    assert_eq!(gone.json()["aliases"], json!([]));
    // Removing an alias not held changes nothing and records nothing.
    let noop = keyed(
        &reads,
        "DELETE",
        &format!("{bob}/bobby"),
        "alias-n",
        json!({}),
    )
    .await;
    assert_eq!(noop.status, 200, "{}", noop.text());
    assert_eq!(member_records(&reads).await.len(), 3);

    // History lists them beside the chain.
    let page = request(
        reads.admin,
        "GET",
        ADMIN_HOST,
        "/api/admin/history",
        &[("Cookie", &reads.cookie)],
    )
    .await;
    assert_eq!(page.status, 200, "{}", page.text());
    let sections: Vec<Value> = page.json()["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["section"].clone())
        .collect();
    assert_eq!(sections, vec![json!("members"); 3]);
}
