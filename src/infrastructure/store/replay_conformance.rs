//! `Idempotency-Key` replays every store must keep: scoped per actor and
//! desk, live until they expire, refusing another request under a live key,
//! written with settings rows (or a portal member edit) in one transaction,
//! and pruned once expired.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use super::fixtures::{refused, refused_for};
use crate::domain::history::{Actor, Surface};
use crate::domain::members::{MemberStore, PingLevel, PortalEdit};
use crate::domain::settings::{RowDiff, SettingsChange, SettingsChangeQuery, SettingsStore, keys};
use crate::infrastructure::store::members_conformance::gateway;
use crate::infrastructure::store::replays::{
    MEMBERS_SECTION, PortalRecord, PortalWrite, REPLAY_TTL, ReplayScope, ReplayStore, StoredReplay,
};

pub async fn run_suite<S: ReplayStore + SettingsStore + MemberStore>(make: impl AsyncFn() -> S) {
    replays_round_trip_per_scope_actor_and_key(make().await).await;
    a_live_key_refuses_another_request(make().await).await;
    expired_keys_read_as_absent_and_apply_anew(make().await).await;
    prune_and_writes_drop_only_expired_rows(make().await).await;
    settings_rows_record_and_replay_commit_together(make().await).await;
    portal_edits_record_and_replay_in_one_write(make().await).await;
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
    refused(store.put_replay(other).await);
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

async fn portal_edits_record_and_replay_in_one_write<
    S: ReplayStore + SettingsStore + MemberStore,
>(
    store: S,
) {
    let name = "portal_edits_record_and_replay_in_one_write";
    store
        .apply_gateway(gateway("200", "Alice", &["20"]))
        .await
        .expect("member");
    let record = PortalRecord {
        at: at(1),
        actor: Actor::admin("token"),
        surface: Surface::AdminPortal,
    };
    let off = || PortalEdit {
        ping_level: Some(PingLevel::Off),
        ..PortalEdit::default()
    };
    let edit = |edit, key: &str, request| {
        store.apply_portal_recorded(
            "200",
            edit,
            record.clone(),
            Some(replay(ReplayScope::Config, key, request, at(1))),
        )
    };
    let saves = async || {
        store
            .settings_changes(SettingsChangeQuery::default())
            .await
            .expect("saves")
    };
    let PortalWrite::Edited(edited) = edit(off(), "k", 'a').await.expect("edit") else {
        panic!("{name}: not edited");
    };
    assert_eq!(edited.member.ping_level, PingLevel::Off, "{name}");
    let recorded = saves().await;
    assert_eq!(recorded.len(), 1, "{name}: one History row");
    let change = &recorded[0];
    assert_eq!(
        (change.section.as_str(), change.revision, &change.actor),
        (MEMBERS_SECTION, 0, &Actor::admin("token")),
        "{name}"
    );
    let row = change.values.get("member.200").expect("member row");
    assert!(row.to.contains(r#""ping_level":"off""#), "{name}: {row:?}");
    assert!(row.from.contains(r#""member":"Alice""#), "{name}: {row:?}");
    assert_ne!(row.from, row.to, "{name}");
    assert!(
        store
            .replay(ReplayScope::Config, "admin:token", "k", at(2))
            .await
            .expect("replay")
            .is_some(),
        "{name}: the key's answer is stored with the edit"
    );

    // The same key and request writes nothing again; another request under
    // the live key is refused and writes nothing.
    let all = PortalEdit {
        ping_level: Some(PingLevel::All),
        ..PortalEdit::default()
    };
    assert_eq!(
        edit(off(), "k", 'a').await.expect("retry"),
        PortalWrite::Replayed,
        "{name}"
    );
    assert_eq!(
        edit(all.clone(), "k", 'b').await.expect("other"),
        PortalWrite::KeyTaken,
        "{name}"
    );
    assert_eq!(saves().await.len(), 1, "{name}: no second row");
    let member = store.load_member("200").await.expect("load").expect("row");
    assert_eq!(
        member.member.ping_level,
        PingLevel::Off,
        "{name}: no second write"
    );

    // A no-op edit records nothing but keeps its answer; a missing member or
    // a refused edit stores neither.
    let PortalWrite::Edited(_) = edit(off(), "k-noop", 'c').await.expect("no-op") else {
        panic!("{name}: no-op not answered");
    };
    assert_eq!(saves().await.len(), 1, "{name}: a no-op records nothing");
    let missing = store
        .apply_portal_recorded(
            "404",
            off(),
            record.clone(),
            Some(replay(ReplayScope::Config, "k-404", 'd', at(1))),
        )
        .await
        .expect("missing");
    assert_eq!(missing, PortalWrite::Missing, "{name}");
    store
        .apply_gateway(gateway("100", "Bob", &["20"]))
        .await
        .expect("member");
    let taken = PortalEdit {
        add_alias: Some("ali".into()),
        ..PortalEdit::default()
    };
    store
        .apply_portal("100", taken.clone())
        .await
        .expect("bob's alias");
    refused_for(
        format!("{name}: the alias is held"),
        edit(taken, "k-taken", 'e').await,
    );
    for key in ["k-404", "k-taken"] {
        assert!(
            store
                .replay(ReplayScope::Config, "admin:token", key, at(2))
                .await
                .expect("replay")
                .is_none(),
            "{name}: {key} stored nothing"
        );
    }
    assert_eq!(saves().await.len(), 1, "{name}");

    // Without a key the edit is still recorded.
    let PortalWrite::Edited(_) = store
        .apply_portal_recorded("200", all, record.clone(), None)
        .await
        .expect("unkeyed")
    else {
        panic!("{name}: unkeyed not edited");
    };
    assert_eq!(saves().await.len(), 2, "{name}: unkeyed edit recorded");
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
        refused(store.put_settings_rows_replayed(rows, change, replay).await);
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
        refused_for(
            format!("replays: {replay:?} is refused"),
            store.put_replay(replay.clone()).await,
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
