//! The behaviour every delivery journal must show, run against stores that
//! are both a [`ScheduleStore`] and a [`DeliveryJournal`] sharing one state.
//! Failures panic with the check name.

use std::collections::BTreeSet;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::bot::delivery::cards::{CardRecord, DigestPhraseStore, PostedCard, ReminderCardStore};
use crate::bot::delivery::{DebugCardStore, PostedDebugCard};
use crate::bot::events::CardIndex;
use crate::domain::notify::{
    AttemptId, AttemptState, Claim, DedupeKey, DeliverySettings, DeliveryTarget, DigestPostInput,
    EffectKind, IntentContent, JournalError, JournalView, Lease, NotificationIntent, PlannedSend,
    REJECTED_ACTOR, Receipt, SendDisposition, WeeklyDigest, plan_digest_post,
};
use crate::domain::notify::{
    DeliveryJournal, DigestAction, NOT_SENT_ACTOR, WeekReset, plan_digest_tick,
};
use crate::domain::schedule::{
    Change, ChangeSet, Draft, Reminder, Run, RunSource, RunStatus, ScheduleSnapshot,
};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::domain::time::to_iso;

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<
    S: ScheduleStore
        + DeliveryJournal
        + CardIndex
        + ReminderCardStore
        + DigestPhraseStore
        + DebugCardStore,
>(
    make: impl AsyncFn() -> S,
) {
    claim_holds_its_targets(&make().await).await;
    concurrent_double_claim_is_held_once(&make().await).await;
    target_gone_or_sent_after_planning_is_refused(&make().await).await;
    bind_stamps_sent_at_and_the_actual_channel(&make().await).await;
    restart_turns_intent_indeterminate_and_suppresses(&make().await).await;
    suppressed_digest_never_touches_replaces(&make().await).await;
    replacement_requires_a_matching_bound_claim(&make().await).await;
    unproven_retired_reminder_never_reopens(&make().await).await;
    rejected_send_is_retired_and_never_retried(&make().await).await;
    notice_retried_within_its_operation_is_held(&make().await).await;
    rejected_digest_raises_the_marker(&make().await).await;
    unproven_digest_raises_the_marker(&make().await).await;
    card_runs_survive_a_reminder_rebuild(&make().await).await;
    unsent_release_frees_the_target_for_a_fresh_claim(&make().await).await;
    digest_week_is_recorded_without_a_post(&make().await).await;
    older_digests_are_retired_and_kept(&make().await).await;
    reminder_card_records_are_written_once_and_found_when_bound(&make().await).await;
    digest_phrase_records_are_written_once(&make().await).await;
    test_cards_are_per_operation_registered_and_cleared(&make().await).await;
}

const HOME: &str = "900";
const FALLBACK: &str = "777";
const INSTANCE: &str = "instance-a";

fn at(hour: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 31, 0, 0, 0)
        .single()
        .expect("valid instant")
        + TimeDelta::hours(hour)
}

fn week() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 26, 16, 0, 0)
        .single()
        .expect("valid instant")
}

fn reminder(id: &str, kind: &str) -> Reminder {
    Reminder {
        id: id.into(),
        run_id: "r-1".into(),
        kind: kind.into(),
        fire_at: at(8),
        sent_at: None,
        message_id: None,
    }
}

async fn seed<S: ScheduleStore>(store: &S) {
    let run = Run {
        id: "r-1".into(),
        fixed_run_id: None,
        channel_id: Some(HOME.into()),
        week_start: week(),
        datetime: at(20),
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    };
    commit(
        store,
        vec![
            Change::PutRun(run),
            Change::PutReminder(reminder("m-1", "day_of")),
            Change::PutReminder(reminder("m-2", "countdown_60")),
        ],
    )
    .await;
}

async fn commit<S: ScheduleStore>(store: &S, changes: Vec<Change>) {
    let revision = snapshot(store).await.revision;
    store
        .commit(
            revision,
            ChangeSet { changes },
            crate::infrastructure::store::conformance::meta(),
        )
        .await
        .expect("schedule commit succeeds");
}

async fn snapshot<S: ScheduleStore>(store: &S) -> ScheduleSnapshot {
    store.load(&Scope::All).await.expect("load succeeds")
}

fn row<'a>(state: &'a ScheduleSnapshot, id: &str) -> &'a Reminder {
    state
        .reminders
        .iter()
        .find(|row| row.id == id)
        .expect("reminder exists")
}

fn intent(targets: &[&str], channel: &str) -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Reminder,
        effect_context: Vec::new(),
        channel_id: channel.into(),
        targets: targets
            .iter()
            .map(|id| DeliveryTarget::Reminder((*id).into()))
            .collect(),
        mentions: vec!["1".into()],
        content: IntentContent::DayOf {
            run_ids: vec!["r-1".into()],
        },
        warnings: Vec::new(),
    }
}

fn digest_intent() -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Digest,
        effect_context: Vec::new(),
        channel_id: HOME.into(),
        targets: vec![DeliveryTarget::Digest(week())],
        mentions: Vec::new(),
        content: IntentContent::Digest {
            week_start: week(),
            inclusion: Default::default(),
        },
        warnings: Vec::new(),
    }
}

async fn lease<S: DeliveryJournal>(store: &S) -> Lease {
    store
        .begin_lease(INSTANCE, "delivery", at(7))
        .await
        .expect("lease begins")
}

async fn fresh<S: DeliveryJournal>(
    store: &S,
    lease: &Lease,
    intent: &NotificationIntent,
) -> AttemptId {
    match store.claim(lease, intent, None, at(8)).await {
        Ok(Claim::Fresh(id)) => id,
        other => panic!("expected a fresh claim: {other:?}"),
    }
}

async fn state<S: DeliveryJournal>(store: &S, attempt: &AttemptId) -> AttemptState {
    store
        .load_attempt(attempt)
        .await
        .expect("load attempt")
        .expect("attempt exists")
        .state
}

fn receipt(channel: &str, message: &str) -> Receipt {
    Receipt {
        channel_id: channel.into(),
        message_id: message.into(),
    }
}

async fn claim_holds_its_targets<S: ScheduleStore + DeliveryJournal>(store: &S) {
    seed(store).await;
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &intent(&["m-1"], HOME)).await;
    assert_eq!(state(store, &attempt).await, AttemptState::Intent);
    let view = store.load_view().await.expect("view");
    assert!(
        view.holds(&DeliveryTarget::Reminder("m-1".into())),
        "claim_holds_its_targets"
    );
    assert!(!view.holds(&DeliveryTarget::Reminder("m-2".into())));
    for held in [intent(&["m-1"], HOME), intent(&["m-2", "m-1"], FALLBACK)] {
        assert_eq!(
            store.claim(&lease, &held, None, at(8)).await,
            Ok(Claim::Held),
            "a held target is never claimed again"
        );
    }
    fresh(store, &lease, &intent(&["m-2"], HOME)).await;
}

async fn concurrent_double_claim_is_held_once<S: ScheduleStore + DeliveryJournal>(store: &S) {
    seed(store).await;
    let lease = lease(store).await;
    let planned = intent(&["m-1", "m-2"], HOME);
    let (first, second) = tokio::join!(
        store.claim(&lease, &planned, None, at(8)),
        store.claim(&lease, &planned, None, at(8)),
    );
    let outcomes = [first.expect("claim"), second.expect("claim")];
    let fresh = outcomes
        .iter()
        .filter(|claim| matches!(claim, Claim::Fresh(_)))
        .count();
    assert_eq!(
        fresh, 1,
        "concurrent_double_claim_is_held_once: {outcomes:?}"
    );
    assert!(outcomes.contains(&Claim::Held));
}

async fn target_gone_or_sent_after_planning_is_refused<S: ScheduleStore + DeliveryJournal>(
    store: &S,
) {
    seed(store).await;
    let lease = lease(store).await;
    let mut sent = reminder("m-2", "countdown_60");
    sent.sent_at = Some(at(8));
    commit(
        store,
        vec![
            Change::DeleteReminder("m-1".into()),
            Change::PutReminder(sent),
        ],
    )
    .await;
    let revision = snapshot(store).await.revision;
    for id in ["m-1", "m-2"] {
        let result = store.claim(&lease, &intent(&[id], HOME), None, at(8)).await;
        assert!(
            matches!(result, Err(JournalError::TargetUnavailable(_))),
            "{id}: {result:?}"
        );
    }
    assert!(store.load_view().await.expect("view").targets().is_empty());
    assert_eq!(
        snapshot(store).await.revision,
        revision,
        "a claim writes no native row"
    );
}

async fn bind_stamps_sent_at_and_the_actual_channel<S: ScheduleStore + DeliveryJournal>(store: &S) {
    seed(store).await;
    let lease = lease(store).await;
    // The run's home channel is unreachable, so the card falls back.
    let attempt = fresh(store, &lease, &intent(&["m-1"], FALLBACK)).await;
    let before = snapshot(store).await;
    let result = store
        .bind(&lease, &attempt, &receipt(HOME, "5001"), None, at(8))
        .await;
    assert!(
        matches!(result, Err(JournalError::StateChanged(_))),
        "a receipt must name the claimed channel: {result:?}"
    );
    assert_eq!(snapshot(store).await, before, "nothing was written");

    store
        .bind(&lease, &attempt, &receipt(FALLBACK, "5001"), None, at(8))
        .await
        .expect("bind_stamps_sent_at_and_the_actual_channel");
    let after = snapshot(store).await;
    assert_eq!(
        after.revision,
        before.revision + 1,
        "binding advances the revision"
    );
    let bound = row(&after, "m-1");
    assert_eq!(bound.sent_at, Some(at(8)));
    assert_eq!(bound.message_id.as_deref(), Some("5001"));
    let record = store
        .load_attempt(&attempt)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(record.state, AttemptState::Bound);
    assert_eq!(record.channel_id, FALLBACK);
    assert_eq!(record.message_id.as_deref(), Some("5001"));
    assert!(store.load_view().await.expect("view").targets().is_empty());
    let again = store
        .bind(&lease, &attempt, &receipt(FALLBACK, "5002"), None, at(9))
        .await;
    assert!(
        matches!(again, Err(JournalError::StateChanged(_))),
        "binds once"
    );
}

async fn restart_turns_intent_indeterminate_and_suppresses<S: ScheduleStore + DeliveryJournal>(
    store: &S,
) {
    seed(store).await;
    let old = lease(store).await;
    let attempt = fresh(store, &old, &intent(&["m-1"], HOME)).await;
    let recovery = store.recover_on_start(at(9)).await.expect("recovery");
    assert_eq!(recovery.indeterminate, std::slice::from_ref(&attempt));
    assert_eq!(recovery.orphaned_leases, 1);
    assert_eq!(state(store, &attempt).await, AttemptState::Indeterminate);
    assert_eq!(
        store
            .claim(&old, &intent(&["m-2"], HOME), None, at(9))
            .await,
        Err(JournalError::LeaseNotLive),
        "an orphaned lease writes nothing"
    );

    let lease = lease(store).await;
    let view = store.load_view().await.expect("view");
    let planned = PlannedSend::new(intent(&["m-1"], HOME), &view);
    assert_eq!(
        planned.disposition,
        SendDisposition::Suppressed,
        "restart_turns_intent_indeterminate_and_suppresses"
    );
    assert_eq!(
        store.claim(&lease, &planned.intent, None, at(9)).await,
        Ok(Claim::Held)
    );
    assert!(row(&snapshot(store).await, "m-1").sent_at.is_none());
}

async fn suppressed_digest_never_touches_replaces<S: ScheduleStore + DeliveryJournal>(store: &S) {
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &digest_intent()).await;
    store
        .mark_indeterminate(&lease, &attempt)
        .await
        .expect("indeterminate");
    let view = store.load_view().await.expect("view");
    // A card the planner believes is live for the week.
    let card = WeeklyDigest {
        week_start: week(),
        channel_id: HOME.into(),
        message_id: "6001".into(),
        posted_at: at(1),
        retired_at: None,
    };
    let channels: BTreeSet<String> = [HOME.to_owned()].into();
    let plan = plan_digest_post(&DigestPostInput {
        week_start: week(),
        current_week: week(),
        zone: chrono_tz::UTC,
        runs: &[],
        digests: std::slice::from_ref(&card),
        explicit_channel: None,
        settings: DeliverySettings {
            post_channel_id: Some(HOME),
            quiet_mode: false,
            attendance: crate::domain::attendance::AttendancePolicy::V4_COMPAT,
        },
        channels: &channels,
        journal: &view,
    })
    .expect("plan")
    .expect("a channel");
    assert_eq!(plan.send.disposition, SendDisposition::Suppressed);
    assert_eq!(plan.replaces.as_ref(), Some(&card));
    let log = store.load_digests().await.expect("digests");
    let result = store.retire_for_replacement(&lease, &card, at(9)).await;
    assert!(
        matches!(result, Err(JournalError::StateChanged(_))),
        "suppressed_digest_never_touches_replaces: {result:?}"
    );
    assert_eq!(store.load_digests().await.expect("digests"), log);
    assert_eq!(state(store, &attempt).await, AttemptState::Indeterminate);
}

async fn replacement_requires_a_matching_bound_claim<S: ScheduleStore + DeliveryJournal>(
    store: &S,
) {
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &digest_intent()).await;
    store
        .bind(
            &lease,
            &attempt,
            &receipt(HOME, "6001"),
            Some(week()),
            at(8),
        )
        .await
        .expect("digest binds");
    let log = store.load_digests().await.expect("digests");
    assert_eq!(
        log.last_digest_week,
        Some(crate::domain::time::to_iso(&week()).unwrap())
    );
    let card = log.digests[0].clone();
    assert_eq!(
        (card.channel_id.as_str(), card.message_id.as_str()),
        (HOME, "6001")
    );
    assert_eq!(
        store.claim(&lease, &digest_intent(), None, at(8)).await,
        Ok(Claim::Held),
        "the bound card holds its week until replaced"
    );
    for wrong in [
        WeeklyDigest {
            message_id: "6002".into(),
            ..card.clone()
        },
        WeeklyDigest {
            channel_id: FALLBACK.into(),
            ..card.clone()
        },
    ] {
        let result = store.retire_for_replacement(&lease, &wrong, at(9)).await;
        assert!(
            matches!(result, Err(JournalError::StateChanged(_))),
            "replacement_requires_a_matching_bound_claim: {result:?}"
        );
    }
    assert_eq!(store.load_digests().await.expect("digests"), log);

    store
        .retire_for_replacement(&lease, &card, at(9))
        .await
        .expect("a confirmed deletion retires the exact claim");
    let retired = store.load_digests().await.expect("digests");
    assert_eq!(retired.digests[0].retired_at, Some(at(9)));
    assert_eq!(state(store, &attempt).await, AttemptState::Retired);
    let next = fresh(store, &lease, &digest_intent()).await;
    assert_ne!(next, attempt);
}

async fn unproven_retired_reminder_never_reopens<S: ScheduleStore + DeliveryJournal>(store: &S) {
    seed(store).await;
    let old = lease(store).await;
    let attempt = fresh(store, &old, &intent(&["m-1"], HOME)).await;
    let early = store
        .retire_unproven(&attempt, "operator:alice", "no evidence either way", at(9))
        .await;
    assert!(
        matches!(early, Err(JournalError::StateChanged(_))),
        "the attempt's own lease must be over: {early:?}"
    );
    store.recover_on_start(at(9)).await.expect("recovery");
    let before = snapshot(store).await.revision;
    store
        .retire_unproven(&attempt, "operator:alice", "no evidence either way", at(9))
        .await
        .expect("operator retirement");
    let state_after = snapshot(store).await;
    assert_eq!(state_after.revision, before + 1);
    assert_eq!(
        state_after.unproven_retired,
        BTreeSet::from(["m-1".to_owned()]),
        "unproven_retired_reminder_never_reopens"
    );
    let handled = row(&state_after, "m-1");
    assert_eq!(
        (handled.sent_at, handled.message_id.as_deref()),
        (Some(at(9)), None)
    );
    let scoped = store
        .load(&Scope::Reminder("m-1".into()))
        .await
        .expect("load");
    assert!(scoped.unproven_retired.contains("m-1"));
    let mut draft = Draft::new(scoped);
    assert!(
        !draft.reschedule_unposted_reminder("m-1", at(30), at(10)),
        "never reopened"
    );
    let lease = lease(store).await;
    let again = store
        .claim(&lease, &intent(&["m-1"], HOME), None, at(10))
        .await;
    assert!(
        matches!(again, Err(JournalError::TargetUnavailable(_))),
        "{again:?}"
    );
    // Even a row some other write put back to unsent is never claimed.
    commit(store, vec![Change::PutReminder(reminder("m-1", "day_of"))]).await;
    let reopened = store
        .claim(&lease, &intent(&["m-1"], HOME), None, at(10))
        .await;
    assert!(
        matches!(reopened, Err(JournalError::TargetUnavailable(_))),
        "{reopened:?}"
    );
}

async fn rejected_send_is_retired_and_never_retried<S: ScheduleStore + DeliveryJournal>(store: &S) {
    seed(store).await;
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &intent(&["m-1"], HOME)).await;
    let empty = store.retire_rejected(&lease, &attempt, "  ", at(8)).await;
    assert!(
        matches!(empty, Err(JournalError::InvalidInput(_))),
        "{empty:?}"
    );
    let before = snapshot(store).await.revision;
    store
        .retire_rejected(&lease, &attempt, "403 Missing Access in 900", at(8))
        .await
        .expect("rejected_send_is_retired_and_never_retried");
    let record = store
        .load_attempt(&attempt)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(record.state, AttemptState::Retired);
    assert_eq!(record.resolved_by.as_deref(), Some(REJECTED_ACTOR));
    assert_eq!(record.resolution_reason, "403 Missing Access in 900");
    assert_eq!(record.message_id, None);
    let after = snapshot(store).await;
    assert_eq!(after.revision, before + 1);
    let handled = row(&after, "m-1");
    assert_eq!(
        (handled.sent_at, handled.message_id.as_deref()),
        (Some(at(8)), None)
    );
    assert!(store.load_view().await.expect("view").targets().is_empty());
    let retry = store
        .claim(&lease, &intent(&["m-1"], HOME), None, at(9))
        .await;
    assert!(
        matches!(retry, Err(JournalError::TargetUnavailable(_))),
        "{retry:?}"
    );
    let twice = store
        .retire_rejected(&lease, &attempt, "again", at(9))
        .await;
    assert!(
        matches!(twice, Err(JournalError::StateChanged(_))),
        "{twice:?}"
    );
}

fn notice() -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Notice("notice.run.status.cancelled".into()),
        effect_context: vec!["r-1".into()],
        channel_id: HOME.into(),
        targets: Vec::new(),
        mentions: Vec::new(),
        content: IntentContent::Countdown {
            run_id: "r-1".into(),
            minutes: 0,
        },
        warnings: Vec::new(),
    }
}

async fn notice_retried_within_its_operation_is_held<S: DeliveryJournal>(store: &S) {
    let lease = lease(store).await;
    let first = store.claim(&lease, &notice(), Some(0), at(8)).await;
    assert!(matches!(first, Ok(Claim::Fresh(_))), "{first:?}");
    assert_eq!(
        store.claim(&lease, &notice(), Some(0), at(8)).await,
        Ok(Claim::Held),
        "notice_retried_within_its_operation_is_held"
    );
    let next = store.claim(&lease, &notice(), None, at(8)).await;
    assert!(
        matches!(next, Ok(Claim::Fresh(_))),
        "a new effect takes ordinal 1"
    );
    for (planned, ordinal) in [(notice(), 5), (notice(), -1), (intent(&["m-1"], HOME), 0)] {
        let refused = store.claim(&lease, &planned, Some(ordinal), at(8)).await;
        assert!(
            matches!(refused, Err(JournalError::InvalidInput(_))),
            "ordinal {ordinal}: {refused:?}"
        );
    }
    let other = lease_named(store).await;
    let fresh = store.claim(&other, &notice(), Some(0), at(8)).await;
    assert!(
        matches!(fresh, Ok(Claim::Fresh(_))),
        "ordinals are per operation: {fresh:?}"
    );
}

async fn lease_named<S: DeliveryJournal>(store: &S) -> Lease {
    store
        .begin_lease("instance-b", "delivery", at(7))
        .await
        .expect("lease begins")
}

/// The weekly tick for the week of [`week`] given the stored marker.
async fn tick<S: DeliveryJournal>(store: &S) -> DigestAction {
    let reset = WeekReset {
        zone: chrono_tz::UTC,
        weekday: chrono::Weekday::Wed,
        time: chrono::NaiveTime::from_hms_opt(16, 0, 0).expect("valid time"),
    };
    let log = store.load_digests().await.expect("digests");
    assert_eq!(
        log.last_digest_week,
        Some(to_iso(&week()).expect("in range")),
        "the retired week is recorded"
    );
    let tick = plan_digest_tick(&reset, at(10), log.last_digest_week.as_deref(), Some(HOME))
        .expect("tick");
    assert_eq!(tick.current_week, week());
    tick.action
}

async fn rejected_digest_raises_the_marker<S: DeliveryJournal>(store: &S) {
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &digest_intent()).await;
    store
        .retire_rejected(&lease, &attempt, "403 Missing Access", at(8))
        .await
        .expect("retired");
    assert_eq!(
        tick(store).await,
        DigestAction::UpToDate,
        "rejected_digest_raises_the_marker: the tick never retries"
    );
}

async fn unproven_digest_raises_the_marker<S: DeliveryJournal>(store: &S) {
    let old = lease(store).await;
    let attempt = fresh(store, &old, &digest_intent()).await;
    store.recover_on_start(at(9)).await.expect("recovery");
    store
        .retire_unproven(&attempt, "operator:alice", "no evidence either way", at(9))
        .await
        .expect("retired");
    assert_eq!(
        tick(store).await,
        DigestAction::UpToDate,
        "unproven_digest_raises_the_marker: the tick never reposts"
    );
}

async fn card_runs_survive_a_reminder_rebuild<S: ScheduleStore + DeliveryJournal + CardIndex>(
    store: &S,
) {
    use twilight_model::id::Id;

    seed(store).await;
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &intent(&["m-1", "m-2"], FALLBACK)).await;
    store
        .bind(&lease, &attempt, &receipt(FALLBACK, "5001"), None, at(8))
        .await
        .expect("bind");
    let runs = |message| store.runs_for_message(Id::new(message));
    assert_eq!(runs(5001).await.expect("lookup"), ["r-1"]);
    commit(
        store,
        vec![
            Change::DeleteReminder("m-1".into()),
            Change::DeleteReminder("m-2".into()),
            Change::PutReminder(reminder("m-new", "day_of")),
        ],
    )
    .await;
    assert_eq!(
        runs(5001).await.expect("lookup"),
        ["r-1"],
        "card_runs_survive_a_reminder_rebuild"
    );
    assert!(runs(5002).await.expect("lookup").is_empty());
}

async fn unsent_release_frees_the_target_for_a_fresh_claim<S: ScheduleStore + DeliveryJournal>(
    store: &S,
) {
    seed(store).await;
    let lease = lease(store).await;
    let other = lease_named(store).await;
    let attempt = fresh(store, &lease, &intent(&["m-1"], HOME)).await;
    let foreign = store
        .release_unsent(&other, &attempt, "429 rate limited", at(8))
        .await;
    assert!(
        matches!(foreign, Err(JournalError::StateChanged(_))),
        "another operation's attempt: {foreign:?}"
    );
    let empty = store.release_unsent(&lease, &attempt, "", at(8)).await;
    assert!(
        matches!(empty, Err(JournalError::InvalidInput(_))),
        "{empty:?}"
    );
    let before = snapshot(store).await;
    store
        .release_unsent(&lease, &attempt, "429 rate limited", at(8))
        .await
        .expect("unsent_release_frees_the_target_for_a_fresh_claim");
    let record = store
        .load_attempt(&attempt)
        .await
        .expect("load")
        .expect("exists");
    assert_eq!(record.state, AttemptState::Retired);
    assert_eq!(record.resolved_by.as_deref(), Some(NOT_SENT_ACTOR));
    assert_eq!(record.message_id, None);
    let after = snapshot(store).await;
    assert_eq!(after, before, "native rows and the revision are untouched");
    assert!(
        after.unproven_retired.is_empty(),
        "a proven-unsent release is not an unproven retirement"
    );
    assert!(store.load_view().await.expect("view").targets().is_empty());
    let scoped = store
        .load(&Scope::Reminder("m-1".into()))
        .await
        .expect("load");
    let mut draft = Draft::new(scoped);
    assert!(
        draft.reschedule_unposted_reminder("m-1", at(30), at(10)),
        "the row still reschedules"
    );
    let again = store.release_unsent(&lease, &attempt, "again", at(9)).await;
    assert!(
        matches!(again, Err(JournalError::StateChanged(_))),
        "retired: {again:?}"
    );

    let retry = fresh(store, &lease, &intent(&["m-1"], HOME)).await;
    store
        .bind(&lease, &retry, &receipt(HOME, "5001"), None, at(9))
        .await
        .expect("the reclaimed reminder binds");
    assert_eq!(
        row(&snapshot(store).await, "m-1").message_id.as_deref(),
        Some("5001")
    );
    let bound = store.release_unsent(&lease, &retry, "late", at(9)).await;
    assert!(
        matches!(bound, Err(JournalError::StateChanged(_))),
        "bound: {bound:?}"
    );

    let unknown = fresh(store, &lease, &intent(&["m-2"], HOME)).await;
    store
        .mark_indeterminate(&lease, &unknown)
        .await
        .expect("indeterminate");
    let indeterminate = store.release_unsent(&lease, &unknown, "late", at(9)).await;
    assert!(
        matches!(indeterminate, Err(JournalError::StateChanged(_))),
        "indeterminate: {indeterminate:?}"
    );
    store.recover_on_start(at(10)).await.expect("recovery");
    let dead = store.release_unsent(&lease, &unknown, "late", at(10)).await;
    assert_eq!(dead, Err(JournalError::LeaseNotLive));
}

async fn digest_week_is_recorded_without_a_post<S: DeliveryJournal>(store: &S) {
    let lease = lease(store).await;
    let earlier = week() - TimeDelta::days(7);
    let later = week() + TimeDelta::days(7);
    store
        .record_digest_week(&lease, earlier, at(8))
        .await
        .expect("record");
    for (step, target) in [(0, week()), (1, week()), (2, earlier), (3, later)] {
        store
            .record_digest_week(&lease, target, at(8))
            .await
            .unwrap_or_else(|error| panic!("step {step}: {error}"));
    }
    assert_eq!(
        tick(store).await,
        DigestAction::UpToDate,
        "digest_week_is_recorded_without_a_post: never backwards, never future"
    );
    assert!(
        store
            .load_digests()
            .await
            .expect("digests")
            .digests
            .is_empty()
    );
    store.recover_on_start(at(9)).await.expect("recovery");
    assert_eq!(
        store.record_digest_week(&lease, week(), at(9)).await,
        Err(JournalError::LeaseNotLive)
    );
}

async fn older_digests_are_retired_and_kept<S: ScheduleStore + DeliveryJournal>(store: &S) {
    let lease = lease(store).await;
    let earlier = week() - TimeDelta::days(7);
    let mut first = digest_intent();
    first.targets = vec![DeliveryTarget::Digest(earlier)];
    let old = fresh(store, &lease, &first).await;
    store
        .bind(&lease, &old, &receipt(HOME, "7001"), None, at(1))
        .await
        .expect("bind");
    let current = fresh(store, &lease, &digest_intent()).await;
    store
        .bind(&lease, &current, &receipt(HOME, "7002"), None, at(2))
        .await
        .expect("bind");
    assert_eq!(
        store.retire_digests_before(&lease, week(), at(3)).await,
        Ok(1),
        "older_digests_are_retired_and_kept"
    );
    assert_eq!(
        store.retire_digests_before(&lease, week(), at(4)).await,
        Ok(0)
    );
    assert_eq!(
        store
            .retire_digests_before(&lease, week() + TimeDelta::days(7), at(5))
            .await,
        Ok(1)
    );
    let log = store.load_digests().await.expect("digests");
    let retired: Vec<_> = log
        .digests
        .iter()
        .map(|row| (row.message_id.as_str(), row.retired_at))
        .collect();
    assert_eq!(
        retired,
        [("7001", Some(at(3))), ("7002", Some(at(5)))],
        "an earlier retirement is kept, as the v4 vector"
    );
    assert_eq!(
        state(store, &old).await,
        AttemptState::Bound,
        "attempts are left alone"
    );
    assert_eq!(state(store, &current).await, AttemptState::Bound);
}

async fn reminder_card_records_are_written_once_and_found_when_bound<
    S: ScheduleStore + DeliveryJournal + ReminderCardStore,
>(
    store: &S,
) {
    seed(store).await;
    let day_of = intent(&["m-1"], HOME);
    let key = DedupeKey::native(&day_of.targets).expect("key");
    let first = CardRecord {
        kind: "day_of".into(),
        heading: Some("Today — Mon 31 Aug".into()),
    };
    assert_eq!(store.card_record(key.as_str()).await.expect("read"), None);
    let saved = store
        .save_card_record(key.as_str(), &first, at(8))
        .await
        .expect("save");
    assert_eq!(saved, first);
    let second = CardRecord {
        kind: "day_of".into(),
        heading: Some("Rise and shine — Mon 31 Aug".into()),
    };
    assert_eq!(
        store
            .save_card_record(key.as_str(), &second, at(9))
            .await
            .expect("save again"),
        first,
        "the first record wins"
    );
    for (bad_key, bad) in [
        ("A".repeat(64), first.clone()),
        (
            key.as_str().to_owned(),
            CardRecord {
                kind: "digest".into(),
                heading: None,
            },
        ),
    ] {
        assert!(
            store.save_card_record(&bad_key, &bad, at(9)).await.is_err(),
            "{bad_key} {bad:?} is refused"
        );
    }
    let countdown_key = "b".repeat(64);
    let countdown = CardRecord {
        kind: "countdown_60".into(),
        heading: Some("Onward!".into()),
    };
    assert_eq!(
        store
            .save_card_record(&countdown_key, &countdown, at(9))
            .await
            .expect("countdown phrase record"),
        countdown
    );
    let changed_phrase = CardRecord {
        kind: "countdown_60".into(),
        heading: Some("Waku waku!".into()),
    };
    assert_eq!(
        store
            .save_card_record(&countdown_key, &changed_phrase, at(10))
            .await
            .expect("save countdown again"),
        countdown,
        "countdown's first phrase wins"
    );
    // Claimed but unbound: nothing to refresh yet.
    let lease = lease(store).await;
    let attempt = fresh(store, &lease, &day_of).await;
    assert!(store.posted_cards("r-1").await.expect("read").is_empty());
    store
        .bind(&lease, &attempt, &receipt(HOME, "5001"), None, at(8))
        .await
        .expect("bind");
    assert_eq!(
        store.posted_cards("r-1").await.expect("read"),
        [PostedCard {
            channel_id: HOME.into(),
            message_id: "5001".into(),
            run_ids: vec!["r-1".into()],
            record: first,
            test: false,
        }]
    );
    assert!(store.posted_cards("r-2").await.expect("read").is_empty());
    // A bound card without a record (posted before records) is not listed.
    let countdown = intent(&["m-2"], HOME);
    let attempt = fresh(store, &lease, &countdown).await;
    store
        .bind(&lease, &attempt, &receipt(HOME, "5002"), None, at(8))
        .await
        .expect("bind");
    assert_eq!(store.posted_cards("r-1").await.expect("read").len(), 1);
}

async fn digest_phrase_records_are_written_once<S: DeliveryJournal + DigestPhraseStore>(store: &S) {
    let key = DedupeKey::native(&[DeliveryTarget::Digest(week())]).expect("native digest key");
    assert_eq!(store.digest_phrase(key.as_str()).await.expect("read"), None);
    let first = "Let's go!";
    assert_eq!(
        store
            .save_digest_phrase(key.as_str(), first, at(8))
            .await
            .expect("save"),
        first
    );
    assert_eq!(
        store
            .save_digest_phrase(key.as_str(), "Waku waku!", at(9))
            .await
            .expect("save again"),
        first,
        "the first phrase wins"
    );
    assert_eq!(
        store.digest_phrase(key.as_str()).await.expect("read saved"),
        Some(first.to_owned())
    );
    assert!(store.save_digest_phrase("bad", first, at(9)).await.is_err());
    assert!(
        store
            .save_digest_phrase(&"c".repeat(64), " ", at(9))
            .await
            .is_err()
    );
    assert!(
        store
            .save_digest_phrase(&"d".repeat(64), &"x".repeat(49), at(9))
            .await
            .is_err()
    );
    assert!(
        store
            .load_digests()
            .await
            .expect("native digest rows")
            .digests
            .is_empty(),
        "phrase storage does not bind or create weekly_digests"
    );
}

fn test_card(run_id: &str, kind: &str) -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::DebugCard,
        effect_context: vec![run_id.into(), kind.into()],
        channel_id: HOME.into(),
        targets: vec![DeliveryTarget::DebugCard {
            run_id: run_id.into(),
            kind: kind.into(),
        }],
        mentions: Vec::new(),
        content: IntentContent::Plain,
        warnings: Vec::new(),
    }
}

/// `/debug ping` cards: operation-scoped (a repeat posts again, a retry in
/// the same operation is held), no held target, registered for the run on
/// bind, reminder rows untouched, listed until cleared.
async fn test_cards_are_per_operation_registered_and_cleared<
    S: ScheduleStore + DeliveryJournal + CardIndex + ReminderCardStore + DebugCardStore,
>(
    store: &S,
) {
    seed(store).await;
    let before = snapshot(store).await.reminders;
    let first_lease = lease(store).await;
    let first = fresh(store, &first_lease, &test_card("r-1", "day_of")).await;
    assert_eq!(
        store
            .claim(&first_lease, &test_card("r-1", "day_of"), Some(0), at(8))
            .await,
        Ok(Claim::Held),
        "the operation already posted it"
    );
    assert!(
        store.load_view().await.expect("view").targets().is_empty(),
        "a test card holds nothing"
    );
    let second_lease = lease_named(store).await;
    let second = fresh(store, &second_lease, &test_card("r-1", "day_of")).await;
    let amend = fresh(store, &second_lease, &test_card("r-1", "amend")).await;
    assert!(matches!(
        store
            .claim(&second_lease, &test_card("r-gone", "day_of"), None, at(8))
            .await,
        Err(JournalError::TargetUnavailable(_))
    ));
    let mut mixed = test_card("r-1", "day_of");
    mixed.targets.push(DeliveryTarget::Reminder("m-1".into()));
    assert!(matches!(
        store.claim(&second_lease, &mixed, None, at(8)).await,
        Err(JournalError::InvalidInput(_))
    ));
    for (lease, attempt, message) in [
        (&first_lease, &first, "7001"),
        (&second_lease, &second, "7002"),
        (&second_lease, &amend, "7003"),
    ] {
        store
            .bind(lease, attempt, &receipt(HOME, message), None, at(8))
            .await
            .expect("bind");
    }
    assert_eq!(
        snapshot(store).await.reminders,
        before,
        "reminder rows untouched"
    );
    let message = |id: &str| twilight_model::id::Id::new(id.parse::<u64>().expect("id"));
    assert_eq!(
        store
            .runs_for_message(message("7003"))
            .await
            .expect("index"),
        ["r-1"],
        "reactions on a test card reach its run"
    );
    let posted = store.posted_cards("r-1").await.expect("posted");
    assert_eq!(
        posted
            .iter()
            .map(|card| (
                card.message_id.as_str(),
                card.test,
                card.record.kind.as_str()
            ))
            .collect::<Vec<_>>(),
        [("7001", true, "day_of"), ("7002", true, "day_of")],
        "day-of/countdown test cards are refreshed; plain texts are not"
    );
    let listed = store.debug_cards_in(HOME, at(7)).await.expect("list");
    assert_eq!(
        listed,
        ["7001", "7002", "7003"]
            .iter()
            .zip(["day_of", "day_of", "amend"])
            .map(|(message, kind)| PostedDebugCard {
                channel_id: HOME.into(),
                message_id: (*message).into(),
                run_id: "r-1".into(),
                kind: kind.into(),
                posted_at: at(8),
            })
            .collect::<Vec<_>>()
    );
    assert!(
        store
            .debug_cards_in(HOME, at(9))
            .await
            .expect("list")
            .is_empty()
    );
    assert!(
        store
            .debug_cards_in(FALLBACK, at(7))
            .await
            .expect("list")
            .is_empty()
    );
    assert!(store.clear_debug_card("7001", at(9)).await.expect("clear"));
    assert!(!store.clear_debug_card("7001", at(9)).await.expect("again"));
    assert!(
        !store
            .clear_debug_card("9999", at(9))
            .await
            .expect("unknown")
    );
    assert_eq!(
        store.debug_cards_in(HOME, at(7)).await.expect("list").len(),
        2
    );
    assert_eq!(
        store
            .posted_cards("r-1")
            .await
            .expect("posted")
            .iter()
            .map(|card| card.message_id.as_str())
            .collect::<Vec<_>>(),
        ["7002"],
        "a cleared card is no longer refreshed"
    );
}
