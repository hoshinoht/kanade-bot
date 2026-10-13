use std::sync::Arc;

use chrono::{NaiveTime, TimeZone, Utc, Weekday};
use kanade::{
    api::write::{ApiClock, FixedPatchLookupGate},
    domain::{
        history::{Actor, Origin, Surface},
        ids::RandomIds,
        members::Roster,
        notify::NoticeOutbox,
        schedule::{FixedEdit, FixedEditChoices, FixedEditRequest, ReminderPolicy, SchedulePolicy},
        scheduler::SchedulerService,
    },
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

use super::{ORIGIN, Reads, timing};
use crate::{
    reads::EDGE_HEADERS,
    support::{ADMIN_HOST, Reply, send},
};

const KALOS: &str = "/api/admin/fixed/f-kalos";

static LOOKUP_GATE_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn other_call(reads: &Reads, method: &str, path: &str, body: Value) -> Reply {
    let (cookie, csrf) = reads.tailscale_session().await;
    let mut headers = vec![
        ("Cookie", cookie.as_str()),
        ORIGIN,
        ("X-Kanade-CSRF", csrf.as_str()),
    ];
    headers.extend_from_slice(&EDGE_HEADERS);
    let body = body.to_string();
    send(reads.admin, method, ADMIN_HOST, path, &headers, Some(&body)).await
}

/// A keyed PATCH of `f-kalos`.
async fn patch(reads: &Reads, body: Value, key: &str) -> Reply {
    reads
        .call("PATCH", KALOS, body, &[("Idempotency-Key", key)])
        .await
}

/// [`patch`] spawned, to be held at the lookup gate.
fn spawn_patch(reads: &Arc<Reads>, body: Value, key: &'static str) -> JoinHandle<Reply> {
    let reads = Arc::clone(reads);
    tokio::spawn(async move { patch(&reads, body, key).await })
}

#[track_caller]
fn ok(reply: &Reply) {
    assert_eq!(reply.status, 200, "{}", reply.text());
}

/// The history head and the outbox notice count.
async fn state(reads: &Reads) -> (u64, usize) {
    let notices = reads.store.outbox_notices().await.unwrap().len();
    (reads.version().await, notices)
}

async fn assert_unchanged(reads: &Reads, (version, notices): (u64, usize)) {
    assert_eq!(reads.version().await, version, "the replay writes nothing");
    assert_eq!(
        reads.store.outbox_notices().await.unwrap().len(),
        notices,
        "the replay enqueues no notice"
    );
}

/// A retry identifies the original full PATCH, even when another admin has
/// subsequently changed one of its fields. The response remains the current
/// row, not a stored historical response.
#[tokio::test]
async fn fixed_patch_replay_survives_an_interleaved_other_admin_edit() {
    let reads = Reads::with_logins().await;
    let version = reads.version().await;
    let original = timing(version, "21:30", "seed");
    let key = "fixed-interleaved-replay";
    ok(&patch(&reads, original.clone(), key).await);

    let interleaved = timing(version + 1, "21:30", "interleaved");
    ok(&other_call(&reads, "PATCH", KALOS, interleaved).await);
    let after_interleave = state(&reads).await;

    let replay = patch(&reads, original, key).await;
    ok(&replay);
    assert_eq!(replay.json()["note"], "interleaved");
    assert_unchanged(&reads, after_interleave).await;
}

/// The retry can begin before the first request commits. Once the first write
/// lands and a participant loses their role, the held handler must recover the
/// recorded identity instead of refusing strict current-roster validation.
#[tokio::test]
async fn concurrently_recorded_fixed_patch_replays_after_roster_loss() {
    let _serial = LOOKUP_GATE_TESTS.lock().await;
    let reads = Arc::new(Reads::with_logins().await);
    let original = timing(reads.version().await, "21:30", "seed");
    let key = "fixed-race-roster";
    let gate = FixedPatchLookupGate::install(key);
    let held = spawn_patch(&reads, original.clone(), key);
    gate.reached().await;

    ok(&patch(&reads, original, key).await);
    reads.demote("1002").await;
    let before = state(&reads).await;
    gate.release();

    let replay = held.await.unwrap();
    ok(&replay);
    assert_eq!(replay.json()["note"], "seed");
    assert_unchanged(&reads, before).await;
}

/// The same initially-fresh race remains a replay when the watched-channel
/// cache disappears before strict validation runs.
#[tokio::test]
async fn concurrently_recorded_fixed_patch_replays_after_watched_channel_loss() {
    let _serial = LOOKUP_GATE_TESTS.lock().await;
    let reads = Arc::new(Reads::with_logins().await);
    let original = timing(reads.version().await, "21:30", "seed");
    let key = "fixed-race-channel";
    let gate = FixedPatchLookupGate::install_lost_channel(key, "kalos-four");
    let held = spawn_patch(&reads, original.clone(), key);
    gate.reached().await;

    ok(&patch(&reads, original, key).await);
    let before = state(&reads).await;
    gate.release();

    let replay = held.await.unwrap();
    ok(&replay);
    assert_eq!(replay.json()["note"], "seed");
    assert_unchanged(&reads, before).await;
}

/// A key that is still unseen after the same boundary never receives the
/// replay-only roster relaxation and remains an ordinary strict refusal.
#[tokio::test]
async fn unseen_fixed_patch_after_roster_loss_stays_strict_and_writes_nothing() {
    let _serial = LOOKUP_GATE_TESTS.lock().await;
    let reads = Arc::new(Reads::with_logins().await);
    let key = "fixed-race-fresh";
    let gate = FixedPatchLookupGate::install(key);
    let held = spawn_patch(&reads, timing(reads.version().await, "21:30", "seed"), key);
    gate.reached().await;
    reads.demote("1002").await;
    let before = state(&reads).await;
    gate.release();

    let refused = held.await.unwrap();
    assert_eq!(refused.error(), (422, "invalid".into()));
    assert_unchanged(&reads, before).await;
}

/// A reused key is refused before a current-row no-op can make a changed body
/// look like a replay.
#[tokio::test]
async fn fixed_patch_reused_key_matching_current_state_is_a_mismatch() {
    let reads = Reads::with_logins().await;
    let version = reads.version().await;
    let key = "fixed-interleaved-mismatch";
    ok(&patch(&reads, timing(version, "21:30", "seed"), key).await);

    let interleaved = timing(version + 1, "20:00", "seed");
    ok(&other_call(&reads, "PATCH", KALOS, interleaved).await);
    let after_interleave = state(&reads).await;

    let reused = patch(&reads, timing(version, "20:00", "seed"), key).await;
    assert_eq!(reused.error(), (422, "idempotency_mismatch".into()));
    assert_unchanged(&reads, after_interleave).await;
}

/// The replay gate runs before the current amended-run set is inspected, so a
/// retry remains safe after another admin invalidates its old choices.
#[tokio::test]
async fn fixed_patch_replay_survives_invalidated_amended_run_choices() {
    let reads = Reads::with_logins().await;
    let body = json!({"day": 5, "time": "23:00", "version": reads.version().await});
    ok(&reads
        .call("POST", "/api/admin/runs/r-kalos/move", body, &[])
        .await);
    let mut original = timing(reads.version().await, "21:30", "seed");
    original["decisions"] = json!({"r-kalos": "keep"});
    let key = "fixed-invalidated-decisions";
    ok(&patch(&reads, original.clone(), key).await);

    let body = json!({"version": reads.version().await});
    ok(&other_call(&reads, "POST", "/api/admin/runs/r-kalos/reset", body).await);
    let before = state(&reads).await;

    ok(&patch(&reads, original, key).await);
    assert_unchanged(&reads, before).await;
}

/// A replay never brings a retired timing back; the unchanged current-row
/// projection remains the normal not-found response.
#[tokio::test]
async fn fixed_patch_replay_after_retirement_is_not_found_without_resurrection() {
    let reads = Reads::with_logins().await;
    let original = timing(reads.version().await, "21:30", "seed");
    let key = "fixed-retired-replay";
    ok(&patch(&reads, original.clone(), key).await);

    ok(&other_call(&reads, "DELETE", KALOS, json!({})).await);
    let before = state(&reads).await;

    let replay = patch(&reads, original, key).await;
    assert_eq!(replay.error(), (404, "not_found".into()));
    assert_unchanged(&reads, before).await;
}

/// Every request field, including no-op metadata, is part of one fixed-PATCH
/// identity. A same-actor key cannot cross a timing or a scheduler operation.
#[tokio::test]
async fn fixed_patch_identity_refuses_metadata_cross_target_and_cross_operation_reuse() {
    let reads = Reads::with_logins().await;
    let version = reads.version().await;
    let original = timing(version, "21:30", "seed");
    let key = "fixed-identity-fields";
    ok(&patch(&reads, original.clone(), key).await);
    let before = state(&reads).await;

    for (name, value) in [
        ("version", json!(version + 1)),
        ("owner_id", json!("1001")),
        ("decisions", json!({"r-kalos": "keep"})),
        ("expect", json!([])),
        ("override", json!([])),
    ] {
        let mut body = original.clone();
        body[name] = value;
        let reply = patch(&reads, body, key).await;
        assert_eq!(
            reply.error(),
            (422, "idempotency_mismatch".into()),
            "{name}"
        );
        assert_unchanged(&reads, before).await;
    }

    let header = [("Idempotency-Key", key)];
    let other_timing = reads
        .call("PATCH", "/api/admin/fixed/other", original, &header)
        .await;
    assert_eq!(other_timing.error(), (422, "idempotency_mismatch".into()));
    let body = json!({"day": 6, "time": "21:00", "version": before.0});
    let other_operation = reads
        .call("POST", "/api/admin/runs/r-kalos/move", body, &header)
        .await;
    assert_eq!(
        other_operation.error(),
        (422, "idempotency_mismatch".into())
    );
    assert_unchanged(&reads, before).await;
}

/// Explicit expectation order is presentation-only; the same semantic form
/// replays even after both fields became current-row no-ops.
#[tokio::test]
async fn fixed_patch_replay_canonicalizes_explicit_expectation_order() {
    let reads = Reads::new().await;
    let first = timing(reads.version().await, "21:30", "first");
    ok(&reads.call("PATCH", KALOS, first, &[]).await);
    let seen = reads.version().await;
    let mut original = timing(seen, "20:00", "second");
    original["expect"] = json!([
        {"field": "time", "seen": seen},
        {"field": "note", "seen": seen}
    ]);
    let key = "fixed-expect-order";
    ok(&patch(&reads, original.clone(), key).await);
    let before = state(&reads).await;

    let mut reordered = original;
    reordered["expect"] = json!([
        {"field": "note", "seen": seen},
        {"field": "time", "seen": seen}
    ]);
    ok(&patch(&reads, reordered, key).await);
    assert_unchanged(&reads, before).await;
}

/// An old scheduler-derived fixed-edit digest cannot establish the full PATCH
/// form, so its key is deliberately refused rather than using a legacy path.
#[tokio::test]
async fn fixed_patch_rejects_a_pre_fix_derived_digest_key() {
    let reads = Reads::new().await;
    let now = Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap();
    let policy = SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    );
    let mut service = SchedulerService::new(
        reads.store.clone(),
        RandomIds,
        ApiClock(Arc::new(move || now)),
    );
    let old_request = FixedEditRequest {
        fixed_id: "f-kalos".into(),
        edit: FixedEdit {
            time: Some(NaiveTime::from_hms_opt(21, 30, 0).unwrap()),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::PerRun(Default::default()),
    };
    let directory = Roster::new();
    service
        .as_origin(
            Origin::new(Actor::admin("token"), Surface::AdminPortal)
                .with_request_id("fixed-legacy-derived"),
        )
        .apply_fixed_edit(&old_request, &directory, &policy)
        .await
        .unwrap();
    let before = state(&reads).await;

    let body = timing(0, "21:30", "bring pots");
    let replay = patch(&reads, body, "fixed-legacy-derived").await;
    assert_eq!(replay.error(), (422, "idempotency_mismatch".into()));
    assert_unchanged(&reads, before).await;
}
