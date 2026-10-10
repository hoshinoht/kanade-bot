//! Decline-notice behaviour shared by SQLite and the in-memory store.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::notify::{DECLINE_NOTICE_COOLDOWN, DeclineNotice, DeclineNoticeStore};
use crate::domain::schedule::{
    Change, ChangeSet, Reminder, Rsvp, RsvpSource, RsvpState, Run, RunSource, RunStatus,
};
use crate::domain::scheduler::{ScheduleStore, StoreError};

/// Run decline-notice checks against a fresh store each time.
pub async fn run_suite<S: ScheduleStore + DeclineNoticeStore>(make: impl AsyncFn() -> S) {
    candidates_commit_atomically_and_replay_is_inert(&make().await).await;
    candidates_require_display_names(&make().await).await;
    bound_rows_are_not_replaced_by_candidates(&make().await).await;
    retraction_and_confirmed_clear_are_exact(&make().await).await;
    cooldown_uses_the_retained_notification_time(&make().await).await;
    recovery_lists_unbound_candidates_and_bound_retractions(&make().await).await;
}

fn at(hour: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("valid instant")
        + TimeDelta::hours(hour)
}

fn run() -> Run {
    Run {
        id: "run-1".into(),
        fixed_run_id: None,
        channel_id: Some("run-channel".into()),
        week_start: at(0),
        datetime: at(20),
        bosses: vec!["HFA".into()],
        participants: vec!["member-1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn candidate(at: DateTime<Utc>) -> DeclineNotice {
    candidate_for("run-1", "member-1", at)
}

fn candidate_for(run_id: &str, user_id: &str, at: DateTime<Utc>) -> DeclineNotice {
    DeclineNotice::candidate(
        run_id,
        user_id,
        Some("source-channel".into()),
        Some("source-message".into()),
        "Decliner",
        at,
    )
}

fn run_for(id: &str) -> Run {
    Run {
        id: id.into(),
        ..run()
    }
}

fn rsvp(at: DateTime<Utc>) -> Change {
    Change::PutRsvp(Rsvp {
        run_id: "run-1".into(),
        user_id: "member-1".into(),
        state: RsvpState::No,
        source: RsvpSource::Reaction,
        at,
    })
}

async fn create<S: DeclineNoticeStore>(store: &S) {
    store
        .commit_with_decline_notices(
            0,
            ChangeSet {
                changes: vec![Change::PutRun(run())],
            },
            super::conformance::meta(),
            vec![candidate(at(1))],
            Vec::new(),
        )
        .await
        .expect("candidate commits");
}

async fn candidates_commit_atomically_and_replay_is_inert<S: ScheduleStore + DeclineNoticeStore>(
    store: &S,
) {
    let refused = store
        .commit_with_decline_notices(
            0,
            ChangeSet {
                changes: vec![Change::PutReminder(Reminder {
                    id: "orphan".into(),
                    run_id: "missing".into(),
                    kind: "day_of".into(),
                    fire_at: at(1),
                    sent_at: None,
                    message_id: None,
                })],
            },
            super::conformance::meta(),
            vec![candidate(at(1))],
            Vec::new(),
        )
        .await;
    assert!(
        matches!(refused, Err(StoreError::Constraint(_))),
        "{refused:?}"
    );
    assert!(
        store
            .decline_notice("run-1", "member-1")
            .await
            .expect("read")
            .is_none(),
        "a refused schedule write has no candidate"
    );

    let meta = crate::domain::history::ChangeMeta {
        origin: crate::domain::history::Origin::for_tests().with_request_id("decline-candidate"),
        request_digest: Some("candidate-a".into()),
        ..super::conformance::meta()
    };
    store
        .commit_with_decline_notices(
            0,
            ChangeSet {
                changes: vec![Change::PutRun(run())],
            },
            meta.clone(),
            vec![candidate(at(1))],
            Vec::new(),
        )
        .await
        .expect("commits");
    store
        .commit_with_decline_notices(
            1,
            ChangeSet {
                changes: vec![rsvp(at(2))],
            },
            meta,
            vec![candidate(at(2))],
            Vec::new(),
        )
        .await
        .expect("replays");
    assert_eq!(
        store
            .decline_notice("run-1", "member-1")
            .await
            .expect("read")
            .expect("candidate")
            .notified_at,
        at(1),
        "a request replay cannot update the candidate"
    );
}

async fn candidates_require_display_names<S: ScheduleStore + DeclineNoticeStore>(store: &S) {
    let mut missing_name = candidate(at(1));
    missing_name.display_name = None;
    let result = store
        .commit_with_decline_notices(
            0,
            ChangeSet {
                changes: vec![Change::PutRun(run())],
            },
            super::conformance::meta(),
            vec![missing_name],
            Vec::new(),
        )
        .await;
    assert!(
        matches!(result, Err(StoreError::Constraint(ref detail)) if detail == "decline notice candidate needs a display name"),
        "{result:?}"
    );
    assert_eq!(
        store
            .load(&crate::domain::scheduler::Scope::All)
            .await
            .expect("load")
            .revision,
        0
    );
}

async fn bound_rows_are_not_replaced_by_candidates<S: ScheduleStore + DeclineNoticeStore>(
    store: &S,
) {
    create(store).await;
    assert!(
        store
            .bind_decline_notice("run-1", "member-1", "actual-channel", "message-1")
            .await
            .expect("bind")
    );
    store
        .commit_with_decline_notices(
            1,
            ChangeSet {
                changes: vec![rsvp(at(2))],
            },
            super::conformance::meta(),
            vec![candidate(at(2))],
            Vec::new(),
        )
        .await
        .expect("schedule state commits while bound candidate stays put");
    let stored = store
        .decline_notice("run-1", "member-1")
        .await
        .expect("read")
        .expect("candidate");
    assert_eq!(stored.message_id.as_deref(), Some("message-1"));
    assert_eq!(stored.notified_at, at(1));
}

async fn retraction_and_confirmed_clear_are_exact<S: DeclineNoticeStore>(store: &S) {
    create(store).await;
    assert!(
        store
            .mark_decline_retract_pending("run-1", "member-1")
            .await
            .expect("marks pending")
    );
    assert!(
        store
            .bind_decline_notice("run-1", "member-1", "actual-channel", "message-1")
            .await
            .expect("bind")
    );
    assert!(
        store
            .decline_notice("run-1", "member-1")
            .await
            .expect("read")
            .expect("candidate")
            .retract_pending,
        "late bind retains the in-flight retraction"
    );
    assert!(
        !store
            .clear_decline_notice_message("run-1", "member-1", "other-message")
            .await
            .expect("wrong receipt is ignored")
    );
    assert!(
        store
            .clear_decline_notice_message("run-1", "member-1", "message-1")
            .await
            .expect("confirmed deletion clears")
    );
    let stored = store
        .decline_notice("run-1", "member-1")
        .await
        .expect("read")
        .expect("candidate");
    assert_eq!(stored.message_id, None);
    assert!(!stored.retract_pending);
    assert_eq!(stored.notified_at, at(1));
}

async fn cooldown_uses_the_retained_notification_time<S: DeclineNoticeStore>(store: &S) {
    create(store).await;
    assert!(
        store
            .decline_notice_on_cooldown(
                "run-1",
                "member-1",
                at(1) + DECLINE_NOTICE_COOLDOWN - TimeDelta::microseconds(1)
            )
            .await
            .expect("checks cooldown")
    );
    assert!(
        !store
            .decline_notice_on_cooldown("run-1", "member-1", at(1) + DECLINE_NOTICE_COOLDOWN)
            .await
            .expect("checks cooldown boundary")
    );
}

async fn recovery_lists_unbound_candidates_and_bound_retractions<
    S: ScheduleStore + DeclineNoticeStore,
>(
    store: &S,
) {
    create(store).await;
    store
        .commit_with_decline_notices(
            1,
            ChangeSet {
                changes: vec![Change::PutRun(run_for("run-2"))],
            },
            super::conformance::meta(),
            vec![candidate_for("run-2", "member-2", at(2))],
            Vec::new(),
        )
        .await
        .expect("second candidate commits");
    assert!(
        store
            .bind_decline_notice("run-2", "member-2", "actual-channel", "message-2")
            .await
            .expect("bind")
    );
    assert!(
        store
            .mark_decline_retract_pending("run-2", "member-2")
            .await
            .expect("retraction marks pending")
    );
    store
        .commit_with_decline_notices(
            2,
            ChangeSet {
                changes: vec![Change::PutRun(run_for("run-3"))],
            },
            super::conformance::meta(),
            vec![candidate_for("run-3", "member-3", at(3))],
            Vec::new(),
        )
        .await
        .expect("third candidate commits");

    let bounded = store
        .pending_decline_notices(2)
        .await
        .expect("recovery reads");
    assert_eq!(
        bounded
            .iter()
            .map(|notice| (notice.run_id.as_str(), notice.user_id.as_str()))
            .collect::<Vec<_>>(),
        [("run-1", "member-1"), ("run-2", "member-2")]
    );
    assert_eq!(bounded[1].message_id.as_deref(), Some("message-2"));
    assert!(bounded[1].retract_pending);
    assert!(
        store
            .pending_decline_notices(0)
            .await
            .expect("zero limit")
            .is_empty()
    );
    assert_eq!(
        store
            .pending_decline_notices(usize::MAX)
            .await
            .expect("hard-capped recovery reads")
            .len(),
        3
    );
}
