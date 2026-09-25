//! The notice outbox every store must keep: notices written in the deciding
//! transaction (commit, merge, draft close, expiry), never on a replay or a
//! refused write, drained once under a lease; and source-keyed journal
//! claims that survive a new lease. Failures panic with the check name.

use chrono::{DateTime, TimeZone, Utc};

use crate::domain::drafts::{
    DraftChange, DraftCreated, DraftEventKind, DraftKind, DraftStore, DraftUpdate, DraftWrite,
    MergeCommit, NewDraft,
};
use crate::domain::history::{Actor, ChangeHistory, ChangeMeta, Origin, Surface};
use crate::domain::model_log::ModelLogStore;
use crate::domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, DrainReason, EffectKind, IntentContent, JournalError,
    NOT_SENT_ACTOR, NoticeOutbox, NotificationIntent, change_source, draft_source,
};
use crate::domain::schedule::{
    Change, ChangeSet, Notice, NoticeChange, RequestDecision, Run, RunSource, RunStatus,
};
use crate::domain::scheduler::{ScheduleStore, Scope, StoreError};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S>(make: impl AsyncFn() -> S)
where
    S: ScheduleStore
        + ChangeHistory
        + DraftStore
        + DeliveryJournal
        + NoticeOutbox
        + ModelLogStore
        + Sync,
{
    commits_write_their_notices_once(make().await).await;
    refused_commits_write_nothing(make().await).await;
    merges_write_their_notices_once(make().await).await;
    closes_and_expiry_write_draft_notices(make().await).await;
    drained_is_final_and_needs_a_lease(make().await).await;
    retention_purges_only_old_drained_notices(make().await).await;
    source_claims_hold_across_leases(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, hour, 0, 0)
        .single()
        .expect("valid instant")
}

fn week() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 26, 16, 0, 0)
        .single()
        .expect("valid instant")
}

fn notice(run_id: &str, to: RunStatus) -> Notice {
    Notice {
        change: NoticeChange::RunStatus {
            run_id: run_id.into(),
            from: RunStatus::Planned,
            to,
        },
        channel_id: Some("900".into()),
        listed: vec!["1".into()],
        via_portal: true,
    }
}

fn decided(request: &str) -> Notice {
    Notice {
        change: NoticeChange::RequestDecided {
            request: request.into(),
            decision: RequestDecision::Expired,
            reason: None,
        },
        channel_id: None,
        listed: vec!["7".into()],
        via_portal: false,
    }
}

fn run(id: &str, hour: u32) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start: week(),
        datetime: at(hour),
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn meta(request_id: &str, outbox: Vec<Notice>) -> ChangeMeta {
    ChangeMeta {
        origin: Origin::new(Actor::admin("root"), Surface::AdminPortal).with_request_id(request_id),
        at: at(2),
        notices: outbox.iter().map(Notice::effect_kind).collect(),
        refs: Vec::new(),
        request_digest: Some(format!("digest-{request_id}")),
        expect: Default::default(),
        outbox,
    }
}

fn puts(id: &str, hour: u32) -> ChangeSet {
    ChangeSet {
        changes: vec![Change::PutRun(run(id, hour))],
    }
}

async fn revision<S: ScheduleStore>(store: &S) -> u64 {
    store.load(&Scope::All).await.expect("load").revision
}

async fn keys<S: NoticeOutbox>(store: &S) -> Vec<(String, i64, Notice)> {
    store
        .outbox_notices()
        .await
        .expect("outbox")
        .into_iter()
        .map(|row| (row.source, row.ordinal, row.notice))
        .collect()
}

fn notice_intent() -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Notice("notice.run.status.cancelled".into()),
        effect_context: vec!["r".into()],
        channel_id: "900".into(),
        targets: Vec::new(),
        mentions: Vec::new(),
        content: IntentContent::Plain,
        warnings: Vec::new(),
    }
}

async fn commits_write_their_notices_once<S: ScheduleStore + NoticeOutbox>(store: S) {
    let notices = vec![
        notice("r-1", RunStatus::Cancelled),
        notice("r-1", RunStatus::Done),
    ];
    let first = store
        .commit(
            revision(&store).await,
            puts("r-1", 10),
            meta("q-1", notices.clone()),
        )
        .await
        .expect("commit")
        .expect("recorded");
    let source = change_source(first.seq);
    let expected: Vec<_> = notices
        .iter()
        .enumerate()
        .map(|(ordinal, notice)| (source.clone(), ordinal as i64, notice.clone()))
        .collect();
    assert_eq!(
        keys(&store).await,
        expected,
        "commit: rows by seq and order"
    );
    let pending = store.pending_notices().await.expect("pending").notices;
    assert_eq!(pending.len(), 2, "commit: both pending");
    assert_eq!(
        pending[0].created_at,
        at(2),
        "commit: stamped with the change"
    );
    assert!(pending.iter().all(|row| row.drained_at.is_none()));

    // An exact retry replays: no new rows, whatever the revision.
    let replay = store
        .commit(0, puts("r-1", 10), meta("q-1", notices.clone()))
        .await
        .expect("replay")
        .expect("recorded");
    assert!(replay.replayed, "commit: replayed");
    assert_eq!(
        keys(&store).await,
        expected,
        "commit: a replay adds nothing"
    );

    // A quiet change writes nothing; the next noisy one takes its own seq.
    store
        .commit(
            revision(&store).await,
            puts("r-2", 11),
            meta("q-2", Vec::new()),
        )
        .await
        .expect("quiet");
    let third = store
        .commit(
            revision(&store).await,
            puts("r-3", 12),
            meta("q-3", vec![notice("r-3", RunStatus::Otot)]),
        )
        .await
        .expect("commit")
        .expect("recorded");
    let all = keys(&store).await;
    assert_eq!(all.len(), 3, "commit: quiet changes write no rows");
    assert_eq!(all[2].0, change_source(third.seq));
    assert_eq!(all[2].1, 0);
}

async fn refused_commits_write_nothing<S: ScheduleStore + NoticeOutbox>(store: S) {
    let stale = store
        .commit(
            revision(&store).await + 5,
            puts("r-1", 10),
            meta("q-1", vec![notice("r-1", RunStatus::Cancelled)]),
        )
        .await;
    assert!(
        matches!(stale, Err(StoreError::Conflict { .. })),
        "{stale:?}"
    );
    // A reminder without a run breaks an invariant: the whole commit goes.
    let orphan = store
        .commit(
            revision(&store).await,
            ChangeSet {
                changes: vec![Change::PutReminder(crate::domain::schedule::Reminder {
                    id: "m-1".into(),
                    run_id: "absent".into(),
                    kind: "countdown_15".into(),
                    fire_at: at(9),
                    sent_at: None,
                    message_id: None,
                })],
            },
            meta("q-2", vec![notice("absent", RunStatus::Cancelled)]),
        )
        .await;
    assert!(orphan.is_err(), "refused: {orphan:?}");
    let empty = store
        .commit(
            revision(&store).await,
            ChangeSet::default(),
            meta("q-3", vec![notice("r", RunStatus::Done)]),
        )
        .await
        .expect("empty");
    assert_eq!(empty, None);
    assert!(keys(&store).await.is_empty(), "refused: nothing written");
}

async fn create<S: ScheduleStore + ChangeHistory + DraftStore>(store: &S, id: &str) -> u64 {
    let base = store.history_head().await.expect("head");
    let revision = revision(store).await;
    let DraftCreated::Created(draft) = store
        .create_draft(NewDraft {
            id: id.into(),
            kind: DraftKind::Admin,
            title: "t".into(),
            author: Actor::admin("root"),
            base,
            base_revision: revision,
            request_type: None,
            subject: None,
            at: at(1),
            request: None,
            submit: None,
        })
        .await
        .expect("create")
    else {
        panic!("draft {id} not created");
    };
    draft.version
}

async fn merges_write_their_notices_once<
    S: ScheduleStore + ChangeHistory + DraftStore + NoticeOutbox,
>(
    store: S,
) {
    let version = create(&store, "d-1").await;
    let merge_meta = || meta("merge:d-1@v1", vec![notice("r-1", RunStatus::Confirmed)]);
    let MergeCommit::Committed(merged) = store
        .commit_merge(
            revision(&store).await,
            puts("r-1", 10),
            merge_meta(),
            "d-1",
            version,
            None,
        )
        .await
        .expect("merge")
    else {
        panic!("merge: stale");
    };
    let rows = keys(&store).await;
    assert_eq!(rows.len(), 1, "merge: one row");
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1),
        (change_source(merged.seq).as_str(), 0)
    );
    let MergeCommit::Committed(replay) = store
        .commit_merge(0, puts("r-1", 10), merge_meta(), "d-1", version, None)
        .await
        .expect("replay")
    else {
        panic!("merge replay: stale");
    };
    assert!(replay.replayed);
    // A stale merge of another draft writes nothing either.
    let other = create(&store, "d-2").await;
    let stale = store
        .commit_merge(
            revision(&store).await,
            puts("r-2", 11),
            meta("merge:d-2@v9", vec![notice("r-2", RunStatus::Done)]),
            "d-2",
            other + 8,
            None,
        )
        .await
        .expect("stale merge");
    assert!(matches!(stale, MergeCommit::Stale(_)));
    assert_eq!(
        keys(&store).await,
        rows,
        "merge: replay and stale add nothing"
    );
}

fn close(draft: &str, version: u64, notices: Vec<Notice>) -> DraftUpdate {
    DraftUpdate {
        draft_id: draft.into(),
        expected_version: version,
        actor: Actor::admin("root"),
        at: at(3),
        change: DraftChange::Close {
            status: crate::domain::drafts::DraftStatus::Rejected,
            reason: Some("no".into()),
            notices,
        },
    }
}

async fn closes_and_expiry_write_draft_notices<
    S: ScheduleStore + ChangeHistory + DraftStore + NoticeOutbox,
>(
    store: S,
) {
    let version = create(&store, "d-1").await;
    let stale = store
        .update_draft(close("d-1", version + 1, vec![decided("d-1")]))
        .await
        .expect("stale close");
    assert!(matches!(stale, DraftWrite::Stale(_)));
    assert!(
        keys(&store).await.is_empty(),
        "close: a stale close writes nothing"
    );
    let closed = store
        .update_draft(close("d-1", version, vec![decided("d-1")]))
        .await
        .expect("close");
    assert!(matches!(closed, DraftWrite::Written(_)));
    assert_eq!(
        keys(&store).await,
        [(draft_source("d-1"), 0, decided("d-1"))],
        "close: keyed by the draft"
    );

    // Expiry writes the planned notice of each draft it expires, only.
    for id in ["d-2", "d-3"] {
        let version = create(&store, id).await;
        let scoped = store
            .update_draft(DraftUpdate {
                draft_id: id.into(),
                expected_version: version,
                actor: Actor::admin("root"),
                at: at(3),
                change: DraftChange::ReplaceOps {
                    ops: Vec::new(),
                    event: DraftEventKind::OpAdded,
                    ord: 0,
                    expires_week: Some(if id == "d-2" {
                        week()
                    } else {
                        week() + chrono::TimeDelta::days(14)
                    }),
                },
            })
            .await
            .expect("scope");
        assert!(matches!(scoped, DraftWrite::Written(_)));
    }
    let expired = store
        .expire_drafts(
            week() + chrono::TimeDelta::days(7),
            at(4),
            &Actor::system("delivery"),
            vec![
                ("d-2".into(), decided("d-2")),
                ("d-3".into(), decided("d-3")),
                ("d-9".into(), decided("d-9")),
            ],
        )
        .await
        .expect("expire");
    assert_eq!(expired, ["d-2"]);
    let rows = keys(&store).await;
    assert_eq!(
        rows[1..],
        [(draft_source("d-2"), 0, decided("d-2"))],
        "expiry: only expired drafts' notices"
    );
}

async fn drained_is_final_and_needs_a_lease<S: ScheduleStore + DeliveryJournal + NoticeOutbox>(
    store: S,
) {
    let committed = store
        .commit(
            revision(&store).await,
            puts("r-1", 10),
            meta(
                "q-1",
                vec![
                    notice("r-1", RunStatus::Cancelled),
                    notice("r-1", RunStatus::Done),
                ],
            ),
        )
        .await
        .expect("commit")
        .expect("recorded");
    let source = change_source(committed.seq);
    let lease = store
        .begin_lease("test", "outbox", at(5))
        .await
        .expect("lease");
    store
        .mark_drained(&lease, &source, 1, DrainReason::Stale, at(5))
        .await
        .expect("drain");
    store
        .mark_drained(&lease, &source, 1, DrainReason::Journal, at(6))
        .await
        .expect("draining again is a no-op");
    let rows = store.outbox_notices().await.expect("outbox");
    assert_eq!(
        rows[1].drained_at,
        Some(at(5)),
        "drained: the first time stands"
    );
    assert_eq!(rows[1].drained_reason, Some(DrainReason::Stale));
    assert_eq!(rows[0].drained_reason, None);
    let pending = store.pending_notices().await.expect("pending").notices;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].ordinal, 0);
    assert!(matches!(
        store
            .mark_drained(&lease, &source, 7, DrainReason::Journal, at(5))
            .await,
        Err(JournalError::StateChanged(_))
    ));
    store.end_lease(&lease, at(5)).await.expect("end");
    assert_eq!(
        store
            .mark_drained(&lease, &source, 0, DrainReason::Journal, at(6))
            .await,
        Err(JournalError::LeaseNotLive),
        "drained: only under a live lease"
    );
    assert_eq!(
        store
            .pending_notices()
            .await
            .expect("pending")
            .notices
            .len(),
        1
    );
}

async fn retention_purges_only_old_drained_notices<
    S: ScheduleStore + DeliveryJournal + NoticeOutbox + ModelLogStore,
>(
    store: S,
) {
    let committed = store
        .commit(
            revision(&store).await,
            puts("r-1", 10),
            meta(
                "q-1",
                vec![
                    notice("r-1", RunStatus::Cancelled),
                    notice("r-1", RunStatus::Done),
                    notice("r-1", RunStatus::Otot),
                ],
            ),
        )
        .await
        .expect("commit")
        .expect("recorded");
    let source = change_source(committed.seq);
    let lease = store
        .begin_lease("test", "outbox", at(5))
        .await
        .expect("lease");
    for (ordinal, hour) in [(0, 5), (1, 9)] {
        store
            .mark_drained(&lease, &source, ordinal, DrainReason::Journal, at(hour))
            .await
            .expect("drain");
    }
    // Pending rows are kept however old; drained ones go by their drain time.
    let pruned = store.prune_model_logs(at(9)).await.expect("prune");
    assert_eq!(
        pruned.notices, 1,
        "retention: one drained before the cutoff"
    );
    let left: Vec<i64> = store
        .outbox_notices()
        .await
        .expect("outbox")
        .into_iter()
        .map(|row| row.ordinal)
        .collect();
    assert_eq!(left, [1, 2]);
    let later = store
        .prune_model_logs(at(9) + chrono::TimeDelta::days(365))
        .await
        .expect("prune");
    assert_eq!(later.notices, 1, "retention: pending rows are never purged");
    assert_eq!(
        store
            .pending_notices()
            .await
            .expect("pending")
            .notices
            .len(),
        1
    );
}

async fn source_claims_hold_across_leases<S: ScheduleStore + DeliveryJournal>(store: S) {
    let intent = notice_intent();
    let first = store
        .begin_lease("test", "tick", at(5))
        .await
        .expect("lease");
    let Claim::Fresh(attempt) = store
        .claim_source(&first, &intent, "change:1", 0, at(5))
        .await
        .expect("claim")
    else {
        panic!("source: first claim held");
    };
    assert_eq!(
        store
            .claim_source(&first, &intent, "change:1", 0, at(5))
            .await,
        Ok(Claim::Held),
        "source: same lease"
    );
    // A crash: the intent becomes indeterminate and a new lease still finds it.
    let recovery = store.recover_on_start(at(6)).await.expect("recover");
    assert_eq!(recovery.indeterminate, [attempt]);
    let second = store
        .begin_lease("test", "tick", at(6))
        .await
        .expect("lease");
    assert_eq!(
        store
            .claim_source(&second, &intent, "change:1", 0, at(6))
            .await,
        Ok(Claim::Held),
        "source: held across leases"
    );
    // Another ordinal or source is its own effect.
    let Claim::Fresh(unsent) = store
        .claim_source(&second, &intent, "change:1", 1, at(6))
        .await
        .expect("claim")
    else {
        panic!("source: other ordinal held");
    };
    store
        .release_unsent(&second, &unsent, "not sent", at(6))
        .await
        .expect("release");
    let Claim::Fresh(rejected) = store
        .claim_source(&second, &intent, "change:1", 1, at(6))
        .await
        .expect("claim")
    else {
        panic!("source: proven unsent stays held");
    };
    let record = store
        .load_attempt(&unsent)
        .await
        .expect("load")
        .expect("row");
    assert_eq!(record.resolved_by.as_deref(), Some(NOT_SENT_ACTOR));
    store
        .retire_rejected(&second, &rejected, "refused", at(6))
        .await
        .expect("retire");
    assert_eq!(
        store
            .claim_source(&second, &intent, "change:1", 1, at(6))
            .await,
        Ok(Claim::Held),
        "source: a rejected notice is never retried"
    );
    let mut targeted = intent.clone();
    targeted.targets = vec![DeliveryTarget::Reminder("m".into())];
    assert!(matches!(
        store
            .claim_source(&second, &targeted, "change:2", 0, at(6))
            .await,
        Err(JournalError::InvalidInput(_))
    ));
}
