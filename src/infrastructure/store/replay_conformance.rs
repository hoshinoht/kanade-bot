//! `Idempotency-Key` replays every store must keep: scoped per actor and
//! desk, live until they expire, refusing another request under a live key,
//! written with settings rows in one transaction, and pruned once expired.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{RowDiff, SettingsChange, SettingsChangeQuery, SettingsStore, keys};
use crate::infrastructure::store::replays::{REPLAY_TTL, ReplayScope, ReplayStore, StoredReplay};

pub async fn run_suite<S: ReplayStore + SettingsStore>(make: impl AsyncFn() -> S) {
    replays_round_trip_per_scope_actor_and_key(make().await).await;
    a_live_key_refuses_another_request(make().await).await;
    expired_keys_read_as_absent_and_apply_anew(make().await).await;
    prune_and_writes_drop_only_expired_rows(make().await).await;
    settings_rows_record_and_replay_commit_together(make().await).await;
    malformed_replays_are_refused(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 9, hour, 0, 0).unwrap()
}

fn digest(byte: char) -> String {
    std::iter::repeat_n(byte, 64).collect()
}

fn replay(scope: ReplayScope, key: &str, request: char, now: DateTime<Utc>) -> StoredReplay {
    StoredReplay::new(
        scope,
        "admin:token".into(),
        key.into(),
        digest(request),
        200,
        r#"{"message":"done"}"#.into(),
        now,
    )
}

async fn replays_round_trip_per_scope_actor_and_key<S: ReplayStore>(store: S) {
    let stored = replay(ReplayScope::Limits, "k-1", 'a', at(1));
    store.put_replay(stored.clone()).await.expect("put");
    let read =
        |scope, actor: &'static str, key: &'static str| store.replay(scope, actor, key, at(2));
    assert_eq!(
        read(ReplayScope::Limits, "admin:token", "k-1")
            .await
            .unwrap(),
        Some(stored),
        "replays: a live key reads back as stored"
    );
    for (scope, actor, key) in [
        (ReplayScope::Rescan, "admin:token", "k-1"),
        (ReplayScope::Config, "admin:token", "k-1"),
        (ReplayScope::Limits, "admin:discord:1", "k-1"),
        (ReplayScope::Limits, "admin:token", "k-2"),
    ] {
        assert_eq!(
            read(scope, actor, key).await.unwrap(),
            None,
            "replays: {scope:?}/{actor}/{key} is another key"
        );
    }
}

async fn a_live_key_refuses_another_request<S: ReplayStore>(store: S) {
    let first = replay(ReplayScope::Config, "k", 'a', at(1));
    store.put_replay(first.clone()).await.expect("put");
    let other = replay(ReplayScope::Config, "k", 'b', at(2));
    assert!(matches!(
        store.put_replay(other).await,
        Err(StoreError::Constraint(_))
    ));
    assert_eq!(
        store
            .replay(ReplayScope::Config, "admin:token", "k", at(2))
            .await
            .unwrap(),
        Some(first.clone()),
        "replays: a refused write leaves the first request"
    );
    // The same request may update its own answer (post-apply notices).
    let mut updated = first;
    updated.body = r#"{"notices":["applied"]}"#.into();
    store.put_replay(updated.clone()).await.expect("update");
    assert_eq!(
        store
            .replay(ReplayScope::Config, "admin:token", "k", at(2))
            .await
            .unwrap(),
        Some(updated)
    );
}

async fn expired_keys_read_as_absent_and_apply_anew<S: ReplayStore>(store: S) {
    let first = replay(ReplayScope::Rescan, "k", 'a', at(1));
    store.put_replay(first.clone()).await.expect("put");
    let expiry = at(1) + REPLAY_TTL;
    let read = |now| store.replay(ReplayScope::Rescan, "admin:token", "k", now);
    assert_eq!(
        read(expiry - TimeDelta::microseconds(1)).await.unwrap(),
        Some(first),
        "replays: live until the instant it expires"
    );
    assert_eq!(read(expiry).await.unwrap(), None, "replays: expired");
    let again = replay(ReplayScope::Rescan, "k", 'b', expiry);
    store
        .put_replay(again.clone())
        .await
        .expect("an expired key takes a new request");
    assert_eq!(read(expiry).await.unwrap(), Some(again));
}

async fn prune_and_writes_drop_only_expired_rows<S: ReplayStore>(store: S) {
    store
        .put_replay(replay(ReplayScope::Limits, "old", 'a', at(0)))
        .await
        .unwrap();
    store
        .put_replay(replay(ReplayScope::Limits, "young", 'a', at(5)))
        .await
        .unwrap();
    let between = at(0) + REPLAY_TTL;
    assert_eq!(store.prune_replays(between).await.unwrap(), 1);
    assert_eq!(store.prune_replays(between).await.unwrap(), 0);
    assert!(
        store
            .replay(ReplayScope::Limits, "admin:token", "young", between)
            .await
            .unwrap()
            .is_some(),
        "replays: a live row survives the prune"
    );
    // A later write drops what expired before it, so nothing is left to prune.
    store
        .put_replay(replay(ReplayScope::Rescan, "late", 'a', at(5) + REPLAY_TTL))
        .await
        .unwrap();
    assert_eq!(
        store.prune_replays(at(5) + REPLAY_TTL).await.unwrap(),
        0,
        "replays: the write pruned the expired row"
    );
    assert!(
        store
            .replay(ReplayScope::Limits, "admin:token", "young", at(6))
            .await
            .unwrap()
            .is_none()
    );
}

fn change(now: DateTime<Utc>) -> SettingsChange {
    SettingsChange {
        id: 0,
        at: now,
        actor: Actor::admin("token"),
        surface: Surface::AdminPortal,
        section: "watching".into(),
        revision: 1,
        values: [(
            keys::PAUSED.to_owned(),
            RowDiff {
                from: "false".into(),
                to: "true".into(),
            },
        )]
        .into(),
    }
}

async fn settings_rows_record_and_replay_commit_together<S: ReplayStore + SettingsStore>(store: S) {
    let rows = || vec![(keys::PAUSED.to_owned(), "true".to_owned())];
    let all = || SettingsChangeQuery::default();
    // Refused: an unknown key, an empty record or a conflicting replay.
    store
        .put_replay(replay(ReplayScope::Config, "taken", 'a', at(1)))
        .await
        .unwrap();
    let mut empty = change(at(1));
    empty.values.clear();
    for (rows, change, replay) in [
        (
            vec![("not.a.setting".to_owned(), "1".to_owned())],
            Some(change(at(1))),
            replay(ReplayScope::Config, "k", 'b', at(1)),
        ),
        (
            rows(),
            Some(empty),
            replay(ReplayScope::Config, "k", 'b', at(1)),
        ),
        (
            rows(),
            Some(change(at(1))),
            replay(ReplayScope::Config, "taken", 'b', at(1)),
        ),
    ] {
        assert!(matches!(
            store.put_settings_rows_replayed(rows, change, replay).await,
            Err(StoreError::Constraint(_))
        ));
    }
    assert!(store.settings_rows().await.unwrap().is_empty());
    assert!(store.settings_changes(all()).await.unwrap().is_empty());
    assert_eq!(
        store
            .replay(ReplayScope::Config, "admin:token", "k", at(1))
            .await
            .unwrap(),
        None,
        "replays: a refused save stores no replay"
    );

    let stored = replay(ReplayScope::Config, "k", 'b', at(1));
    store
        .put_settings_rows_replayed(rows(), Some(change(at(1))), stored.clone())
        .await
        .expect("save");
    assert_eq!(
        store
            .settings_rows()
            .await
            .unwrap()
            .get(keys::PAUSED)
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(store.settings_changes(all()).await.unwrap().len(), 1);
    assert_eq!(
        store
            .replay(ReplayScope::Config, "admin:token", "k", at(1))
            .await
            .unwrap(),
        Some(stored)
    );
    // A row-less record with its replay (a cleared Limits window).
    let cleared = replay(ReplayScope::Limits, "clear", 'c', at(2));
    store
        .put_settings_rows_replayed(Vec::new(), Some(change(at(2))), cleared.clone())
        .await
        .expect("record");
    assert_eq!(store.settings_changes(all()).await.unwrap().len(), 2);
    assert_eq!(
        store
            .replay(ReplayScope::Limits, "admin:token", "clear", at(2))
            .await
            .unwrap(),
        Some(cleared)
    );
}

async fn malformed_replays_are_refused<S: ReplayStore>(store: S) {
    let good = || replay(ReplayScope::Limits, "k", 'a', at(1));
    let mut bad = Vec::new();
    let mut status = good();
    status.status = 500;
    bad.push(status);
    let mut body = good();
    body.body = "not json".into();
    bad.push(body);
    let mut big = good();
    big.body = serde_json::json!({ "message": "x".repeat(16 * 1024) }).to_string();
    bad.push(big);
    let mut request = good();
    request.request = digest('A');
    bad.push(request);
    let mut key = good();
    key.key = "k".repeat(129);
    bad.push(key);
    let mut actor = good();
    actor.actor = String::new();
    bad.push(actor);
    let mut expiry = good();
    expiry.expires_at = expiry.created_at;
    bad.push(expiry);
    for replay in bad {
        assert!(
            matches!(
                store.put_replay(replay.clone()).await,
                Err(StoreError::Constraint(_))
            ),
            "replays: {replay:?} is refused"
        );
    }
    assert_eq!(
        store
            .replay(ReplayScope::Limits, "admin:token", "k", at(1))
            .await
            .unwrap(),
        None
    );
}
