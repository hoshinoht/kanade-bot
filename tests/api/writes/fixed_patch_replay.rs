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

use super::{ORIGIN, Reads, timing};
use crate::{
    reads::EDGE_HEADERS,
    support::{ADMIN_HOST, Reply, send},
};

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

async fn assert_unchanged(reads: &Reads, version: u64, notices: usize) {
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
    let key = [("Idempotency-Key", "fixed-interleaved-replay")];
    let first = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original.clone(), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());

    let other = other_call(
        &reads,
        "PATCH",
        "/api/admin/fixed/f-kalos",
        timing(version + 1, "21:30", "interleaved"),
    )
    .await;
    assert_eq!(other.status, 200, "{}", other.text());
    let after_interleave = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let replay = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original, &key)
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["note"], "interleaved");
    assert_unchanged(&reads, after_interleave, notices).await;
}

/// The retry can begin before the first request commits. Once the first write
/// lands and a participant loses their role, the held handler must recover the
/// recorded identity instead of refusing strict current-roster validation.
#[tokio::test]
async fn concurrently_recorded_fixed_patch_replays_after_roster_loss() {
    let _serial = LOOKUP_GATE_TESTS.lock().await;
    let reads = Arc::new(Reads::with_logins().await);
    let version = reads.version().await;
    let original = timing(version, "21:30", "seed");
    let key = "fixed-race-roster";
    let gate = FixedPatchLookupGate::install(key);
    let held = {
        let reads = Arc::clone(&reads);
        let original = original.clone();
        tokio::spawn(async move {
            reads
                .call(
                    "PATCH",
                    "/api/admin/fixed/f-kalos",
                    original,
                    &[("Idempotency-Key", key)],
                )
                .await
        })
    };
    gate.reached().await;

    let first = reads
        .call(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            original,
            &[("Idempotency-Key", key)],
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    reads.demote("1002").await;
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();
    gate.release();

    let replay = held.await.unwrap();
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_eq!(replay.json()["note"], "seed");
    assert_unchanged(&reads, revision, notices).await;
}

/// A key that is still unseen after the same boundary never receives the
/// replay-only roster relaxation and remains an ordinary strict refusal.
#[tokio::test]
async fn unseen_fixed_patch_after_roster_loss_stays_strict_and_writes_nothing() {
    let _serial = LOOKUP_GATE_TESTS.lock().await;
    let reads = Arc::new(Reads::with_logins().await);
    let version = reads.version().await;
    let key = "fixed-race-fresh";
    let gate = FixedPatchLookupGate::install(key);
    let held = {
        let reads = Arc::clone(&reads);
        tokio::spawn(async move {
            reads
                .call(
                    "PATCH",
                    "/api/admin/fixed/f-kalos",
                    timing(version, "21:30", "seed"),
                    &[("Idempotency-Key", key)],
                )
                .await
        })
    };
    gate.reached().await;
    reads.demote("1002").await;
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();
    gate.release();

    let refused = held.await.unwrap();
    assert_eq!(
        (refused.status, refused.api_error()),
        (422, "invalid".into())
    );
    assert_unchanged(&reads, revision, notices).await;
}

/// A reused key is refused before a current-row no-op can make a changed body
/// look like a replay.
#[tokio::test]
async fn fixed_patch_reused_key_matching_current_state_is_a_mismatch() {
    let reads = Reads::with_logins().await;
    let version = reads.version().await;
    let original = timing(version, "21:30", "seed");
    let key = [("Idempotency-Key", "fixed-interleaved-mismatch")];
    let first = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original, &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());

    let other = other_call(
        &reads,
        "PATCH",
        "/api/admin/fixed/f-kalos",
        timing(version + 1, "20:00", "seed"),
    )
    .await;
    assert_eq!(other.status, 200, "{}", other.text());
    let after_interleave = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let reused = reads
        .call(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            timing(version, "20:00", "seed"),
            &key,
        )
        .await;
    assert_eq!(reused.status, 422, "{}", reused.text());
    assert_eq!(reused.api_error(), "idempotency_mismatch");
    assert_unchanged(&reads, after_interleave, notices).await;
}

/// The replay gate runs before the current amended-run set is inspected, so a
/// retry remains safe after another admin invalidates its old choices.
#[tokio::test]
async fn fixed_patch_replay_survives_invalidated_amended_run_choices() {
    let reads = Reads::with_logins().await;
    let moved = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 5, "time": "23:00", "version": reads.version().await}),
            &[],
        )
        .await;
    assert_eq!(moved.status, 200, "{}", moved.text());
    let version = reads.version().await;
    let mut original = timing(version, "21:30", "seed");
    original["decisions"] = json!({"r-kalos": "keep"});
    let key = [("Idempotency-Key", "fixed-invalidated-decisions")];
    let first = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original.clone(), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());

    let reset = other_call(
        &reads,
        "POST",
        "/api/admin/runs/r-kalos/reset",
        json!({"version": reads.version().await}),
    )
    .await;
    assert_eq!(reset.status, 200, "{}", reset.text());
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let replay = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original, &key)
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_unchanged(&reads, revision, notices).await;
}

/// A replay never brings a retired timing back; the unchanged current-row
/// projection remains the normal not-found response.
#[tokio::test]
async fn fixed_patch_replay_after_retirement_is_not_found_without_resurrection() {
    let reads = Reads::with_logins().await;
    let original = timing(reads.version().await, "21:30", "seed");
    let key = [("Idempotency-Key", "fixed-retired-replay")];
    let first = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original.clone(), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());

    let retired = other_call(&reads, "DELETE", "/api/admin/fixed/f-kalos", json!({})).await;
    assert_eq!(retired.status, 200, "{}", retired.text());
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let replay = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original, &key)
        .await;
    assert_eq!(
        (replay.status, replay.api_error()),
        (404, "not_found".into())
    );
    assert_unchanged(&reads, revision, notices).await;
}

/// Every request field, including no-op metadata, is part of one fixed-PATCH
/// identity. A same-actor key cannot cross a timing or a scheduler operation.
#[tokio::test]
async fn fixed_patch_identity_refuses_metadata_cross_target_and_cross_operation_reuse() {
    let reads = Reads::with_logins().await;
    let version = reads.version().await;
    let original = timing(version, "21:30", "seed");
    let key = [("Idempotency-Key", "fixed-identity-fields")];
    let first = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original.clone(), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let mut changed_version = original.clone();
    changed_version["version"] = (version + 1).into();
    let mut changed_owner = original.clone();
    changed_owner["owner_id"] = "1001".into();
    let mut changed_decisions = original.clone();
    changed_decisions["decisions"] = json!({"r-kalos": "keep"});
    let mut changed_expect = original.clone();
    changed_expect["expect"] = json!([]);
    let mut changed_override = original.clone();
    changed_override["override"] = json!([]);
    for (name, body) in [
        ("version", changed_version),
        ("owner", changed_owner),
        ("decisions", changed_decisions),
        ("expect", changed_expect),
        ("override", changed_override),
    ] {
        let reply = reads
            .call("PATCH", "/api/admin/fixed/f-kalos", body, &key)
            .await;
        assert_eq!(
            (reply.status, reply.api_error()),
            (422, "idempotency_mismatch".into()),
            "{name}"
        );
        assert_unchanged(&reads, revision, notices).await;
    }

    let other_timing = reads
        .call("PATCH", "/api/admin/fixed/other", original, &key)
        .await;
    assert_eq!(
        (other_timing.status, other_timing.api_error()),
        (422, "idempotency_mismatch".into())
    );
    let other_operation = reads
        .call(
            "POST",
            "/api/admin/runs/r-kalos/move",
            json!({"day": 6, "time": "21:00", "version": revision}),
            &key,
        )
        .await;
    assert_eq!(
        (other_operation.status, other_operation.api_error()),
        (422, "idempotency_mismatch".into())
    );
    assert_unchanged(&reads, revision, notices).await;
}

/// Explicit expectation order is presentation-only; the same semantic form
/// replays even after both fields became current-row no-ops.
#[tokio::test]
async fn fixed_patch_replay_canonicalizes_explicit_expectation_order() {
    let reads = Reads::new().await;
    let version = reads.version().await;
    let seed = reads
        .call(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            timing(version, "21:30", "first"),
            &[],
        )
        .await;
    assert_eq!(seed.status, 200, "{}", seed.text());
    let seen = reads.version().await;
    let mut original = timing(seen, "20:00", "second");
    original["expect"] = json!([
        {"field": "time", "seen": seen},
        {"field": "note", "seen": seen}
    ]);
    let key = [("Idempotency-Key", "fixed-expect-order")];
    let first = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", original.clone(), &key)
        .await;
    assert_eq!(first.status, 200, "{}", first.text());
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let mut reordered = original;
    reordered["expect"] = json!([
        {"field": "note", "seen": seen},
        {"field": "time", "seen": seen}
    ]);
    let replay = reads
        .call("PATCH", "/api/admin/fixed/f-kalos", reordered, &key)
        .await;
    assert_eq!(replay.status, 200, "{}", replay.text());
    assert_unchanged(&reads, revision, notices).await;
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
    let revision = reads.version().await;
    let notices = reads.store.outbox_notices().await.unwrap().len();

    let replay = reads
        .call(
            "PATCH",
            "/api/admin/fixed/f-kalos",
            timing(0, "21:30", "bring pots"),
            &[("Idempotency-Key", "fixed-legacy-derived")],
        )
        .await;
    assert_eq!(
        (replay.status, replay.api_error()),
        (422, "idempotency_mismatch".into())
    );
    assert_unchanged(&reads, revision, notices).await;
}
