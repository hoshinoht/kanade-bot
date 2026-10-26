//! The notice outbox every store must keep: notices written in the deciding
//! transaction (commit, merge, draft close, expiry), never on a replay or a
//! refused write, drained once under a lease, and paged in write order; and
//! source-keyed journal claims that survive a new lease. Failures panic with
//! the check name.

use chrono::{DateTime, TimeDelta, Utc};

use super::fixtures::utc2026;
use kanade_domain::drafts::{
    DraftChange, DraftCreated, DraftEventKind, DraftKind, DraftStore, DraftUpdate, DraftWrite,
    MergeCommit, NewDraft,
};
use kanade_domain::history::{Actor, ChangeHistory, ChangeMeta, Origin, Surface};
use kanade_domain::model_log::ModelLogStore;
use kanade_domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, DrainReason, EffectKind, IntentContent, JournalError,
    Lease, NOT_SENT_ACTOR, NoticeOutbox, NotificationIntent, PendingNotices, change_source,
    draft_source,
};
use kanade_domain::schedule::{
    Change, ChangeSet, Notice, NoticeChange, RequestDecision, Run, RunSource, RunStatus,
};
use kanade_domain::scheduler::{Committed, ScheduleStore, Scope, StoreError};

/// Run every check, each against a fresh store from `make`. `undecodable`
/// writes one pending row `(source, 0)` after every row so far, whose payload
/// this build cannot decode (a store's API never writes one).
pub async fn run_suite<S>(make: impl AsyncFn() -> S, undecodable: impl AsyncFn(&S, &str))
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
    pending_pages_in_write_order(make().await, undecodable).await;
    source_claims_hold_across_leases(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    utc2026(9, 1, hour, 0)
}

fn week() -> DateTime<Utc> {
    utc2026(8, 26, 16, 0)
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

/// `r-1`'s status notices, one per status, in order.
fn notices(statuses: &[RunStatus]) -> Vec<Notice> {
    statuses.iter().map(|to| notice("r-1", *to)).collect()
}

fn decided(request: &str) -> Notice {
    Notice {
        change: NoticeChange::RequestDecided {
            request: request.into(),
            decision: RequestDecision::Expired,
            reason: None,
            edited: None,
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

/// Commit `changes` at the current revision.
async fn commit<S: ScheduleStore>(
    store: &S,
    changes: ChangeSet,
    meta: ChangeMeta,
) -> Result<Option<Committed>, StoreError> {
    store.commit(revision(store).await, changes, meta).await
}

/// [`commit`], which the store must record.
async fn recorded<S: ScheduleStore>(store: &S, changes: ChangeSet, meta: ChangeMeta) -> Committed {
    let committed = commit(store, changes, meta).await.expect("commit");
    committed.expect("recorded")
}

async fn keys<S: NoticeOutbox>(store: &S) -> Vec<(String, i64, Notice)> {
    let rows = store.outbox_notices().await.expect("outbox");
    let keys = rows
        .into_iter()
        .map(|row| (row.source, row.ordinal, row.notice));
    keys.collect()
}

async fn pending<S: NoticeOutbox>(store: &S) -> usize {
    store
        .pending_notices()
        .await
        .expect("pending")
        .notices
        .len()
}

async fn lease<S: DeliveryJournal>(store: &S, operation: &str, hour: u32) -> Lease {
    let lease = store.begin_lease("test", operation, at(hour)).await;
    lease.expect("lease")
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
    let notices = notices(&[RunStatus::Cancelled, RunStatus::Done]);
    let first = recorded(&store, puts("r-1", 10), meta("q-1", notices.clone())).await;
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
    let replay = store.commit(0, puts("r-1", 10), meta("q-1", notices)).await;
    assert!(
        replay.expect("replay").expect("recorded").replayed,
        "commit: replayed"
    );
    assert_eq!(
        keys(&store).await,
        expected,
        "commit: a replay adds nothing"
    );

    // A quiet change writes nothing; the next noisy one takes its own seq.
    commit(&store, puts("r-2", 11), meta("q-2", Vec::new()))
        .await
        .expect("quiet");
    let otot = meta("q-3", vec![notice("r-3", RunStatus::Otot)]);
    let third = recorded(&store, puts("r-3", 12), otot).await;
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
    let orphan = ChangeSet {
        changes: vec![Change::PutReminder(kanade_domain::schedule::Reminder {
            id: "m-1".into(),
            run_id: "absent".into(),
            kind: "countdown_15".into(),
            fire_at: at(9),
            sent_at: None,
            message_id: None,
        })],
    };
    let orphan_meta = meta("q-2", vec![notice("absent", RunStatus::Cancelled)]);
    let orphan = commit(&store, orphan, orphan_meta).await;
    assert!(orphan.is_err(), "refused: {orphan:?}");
    let empty_meta = meta("q-3", vec![notice("r", RunStatus::Done)]);
    let empty = commit(&store, ChangeSet::default(), empty_meta).await;
    assert_eq!(empty.expect("empty"), None);
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
    let base = revision(&store).await;
    let merge = store.commit_merge(base, puts("r-1", 10), merge_meta(), "d-1", version, None);
    let MergeCommit::Committed(merged) = merge.await.expect("merge") else {
        panic!("merge: stale");
    };
    let rows = keys(&store).await;
    assert_eq!(rows.len(), 1, "merge: one row");
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1),
        (change_source(merged.seq).as_str(), 0)
    );
    let replay = store.commit_merge(0, puts("r-1", 10), merge_meta(), "d-1", version, None);
    let MergeCommit::Committed(replay) = replay.await.expect("replay") else {
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

/// Admin `root`'s `change` to `draft` at `version`.
fn update(draft: &str, version: u64, change: DraftChange) -> DraftUpdate {
    DraftUpdate {
        draft_id: draft.into(),
        expected_version: version,
        actor: Actor::admin("root"),
        at: at(3),
        change,
    }
}

fn close(draft: &str, version: u64) -> DraftUpdate {
    let change = DraftChange::Close {
        status: kanade_domain::drafts::DraftStatus::Rejected,
        reason: Some("no".into()),
        notices: vec![decided(draft)],
    };
    update(draft, version, change)
}

async fn closes_and_expiry_write_draft_notices<
    S: ScheduleStore + ChangeHistory + DraftStore + NoticeOutbox,
>(
    store: S,
) {
    let version = create(&store, "d-1").await;
    let stale = store.update_draft(close("d-1", version + 1)).await;
    assert!(matches!(stale.expect("stale close"), DraftWrite::Stale(_)));
    assert!(
        keys(&store).await.is_empty(),
        "close: a stale close writes nothing"
    );
    let closed = store.update_draft(close("d-1", version)).await;
    assert!(matches!(closed.expect("close"), DraftWrite::Written(_)));
    assert_eq!(
        keys(&store).await,
        [(draft_source("d-1"), 0, decided("d-1"))],
        "close: keyed by the draft"
    );

    // Expiry writes the planned notice of each draft it expires, only.
    for (id, expires) in [("d-2", week()), ("d-3", week() + TimeDelta::days(14))] {
        let version = create(&store, id).await;
        let scope = DraftChange::ReplaceOps {
            ops: Vec::new(),
            event: DraftEventKind::OpAdded,
            ord: 0,
            expires_week: Some(expires),
        };
        let scoped = store.update_draft(update(id, version, scope)).await;
        assert!(matches!(scoped.expect("scope"), DraftWrite::Written(_)));
    }
    let expired = store
        .expire_drafts(
            week() + TimeDelta::days(7),
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
    let two = meta("q-1", notices(&[RunStatus::Cancelled, RunStatus::Done]));
    let source = change_source(recorded(&store, puts("r-1", 10), two).await.seq);
    let lease = lease(&store, "outbox", 5).await;
    let drain = async |ordinal, reason, hour| {
        let drained = store.mark_drained(&lease, &source, ordinal, reason, at(hour));
        drained.await
    };
    drain(1, DrainReason::Stale, 5).await.expect("drain");
    drain(1, DrainReason::Journal, 6)
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
    let pending_rows = store.pending_notices().await.expect("pending").notices;
    assert_eq!(pending_rows.len(), 1);
    assert_eq!(pending_rows[0].ordinal, 0);
    assert!(matches!(
        drain(7, DrainReason::Journal, 5).await,
        Err(JournalError::StateChanged(_))
    ));
    store.end_lease(&lease, at(5)).await.expect("end");
    assert_eq!(
        drain(0, DrainReason::Journal, 6).await,
        Err(JournalError::LeaseNotLive),
        "drained: only under a live lease"
    );
    assert_eq!(pending(&store).await, 1);
}

async fn retention_purges_only_old_drained_notices<
    S: ScheduleStore + DeliveryJournal + NoticeOutbox + ModelLogStore,
>(
    store: S,
) {
    let three = notices(&[RunStatus::Cancelled, RunStatus::Done, RunStatus::Otot]);
    let committed = recorded(&store, puts("r-1", 10), meta("q-1", three)).await;
    let source = change_source(committed.seq);
    let lease = lease(&store, "outbox", 5).await;
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
    let left: Vec<i64> = keys(&store).await.iter().map(|key| key.1).collect();
    assert_eq!(left, [1, 2]);
    let later = store.prune_model_logs(at(9) + TimeDelta::days(365)).await;
    assert_eq!(
        later.expect("prune").notices,
        1,
        "retention: pending rows are never purged"
    );
    assert_eq!(pending(&store).await, 1);
}

type Keys = Vec<(String, i64)>;

/// One page's decoded and undecodable keys, and whether it has a next page.
fn page_keys(page: &PendingNotices) -> (Keys, Keys, bool) {
    let decoded = page.notices.iter();
    let bad = page.undecodable.iter();
    (
        decoded
            .map(|row| (row.source.clone(), row.ordinal))
            .collect(),
        bad.map(|row| (row.source.clone(), row.ordinal)).collect(),
        page.next.is_some(),
    )
}

async fn pending_pages_in_write_order<S: ScheduleStore + DeliveryJournal + NoticeOutbox>(
    store: S,
    undecodable: impl AsyncFn(&S, &str),
) {
    let three = meta(
        "q-1",
        notices(&[RunStatus::Cancelled, RunStatus::Done, RunStatus::Otot]),
    );
    let first = change_source(recorded(&store, puts("r-1", 10), three).await.seq);
    let bad = "draft:undecodable";
    undecodable(&store, bad).await;
    let two = meta("q-2", notices(&[RunStatus::Cancelled, RunStatus::Done]));
    let second = change_source(recorded(&store, puts("r-2", 11), two).await.seq);
    let lease = lease(&store, "outbox", 5).await;
    store
        .mark_drained(&lease, &first, 1, DrainReason::Journal, at(5))
        .await
        .expect("drain");
    // Pending in write order: first#0, first#2, bad#0, second#0, second#1.
    let key = |source: &str, ordinal: i64| (source.to_owned(), ordinal);
    let mut pages = Vec::new();
    let mut after = None;
    loop {
        let page = store.pending_notices_after(after, 2).await.expect("page");
        pages.push(page_keys(&page));
        let Some(next) = page.next else { break };
        after = Some(next);
    }
    assert_eq!(
        pages,
        [
            (vec![key(&first, 0), key(&first, 2)], vec![], true),
            (vec![key(&second, 0)], vec![key(bad, 0)], true),
            (vec![key(&second, 1)], vec![], false),
        ],
        "paging: every pending row once, in write order, past an undecodable one"
    );
    // A page that ends exactly at the last row still has a next one: empty.
    let all = store.pending_notices_after(None, 5).await.expect("page");
    assert!(all.next.is_some(), "paging: a full page points on");
    let rest = store.pending_notices_after(all.next, 5).await;
    assert_eq!(
        page_keys(&rest.expect("page")),
        (vec![], vec![], false),
        "paging: nothing after the last row"
    );
    let whole = store.pending_notices().await.expect("pending");
    assert_eq!(
        page_keys(&whole),
        (
            vec![
                key(&first, 0),
                key(&first, 2),
                key(&second, 0),
                key(&second, 1)
            ],
            vec![key(bad, 0)],
            false
        ),
        "paging: the one-read view agrees"
    );
}

async fn source_claims_hold_across_leases<S: ScheduleStore + DeliveryJournal>(store: S) {
    let intent = notice_intent();
    let claim = async |lease: &Lease, ordinal, hour| {
        let claim = store.claim_source(lease, &intent, "change:1", ordinal, at(hour));
        claim.await
    };
    let first = lease(&store, "tick", 5).await;
    let Claim::Fresh(attempt) = claim(&first, 0, 5).await.expect("claim") else {
        panic!("source: first claim held");
    };
    assert_eq!(
        claim(&first, 0, 5).await,
        Ok(Claim::Held),
        "source: same lease"
    );
    // A crash: the intent becomes indeterminate and a new lease still finds it.
    let recovery = store.recover_on_start(at(6)).await.expect("recover");
    assert_eq!(recovery.indeterminate, [attempt]);
    let second = lease(&store, "tick", 6).await;
    assert_eq!(
        claim(&second, 0, 6).await,
        Ok(Claim::Held),
        "source: held across leases"
    );
    // Another ordinal or source is its own effect.
    let Claim::Fresh(unsent) = claim(&second, 1, 6).await.expect("claim") else {
        panic!("source: other ordinal held");
    };
    store
        .release_unsent(&second, &unsent, "not sent", at(6))
        .await
        .expect("release");
    let Claim::Fresh(rejected) = claim(&second, 1, 6).await.expect("claim") else {
        panic!("source: proven unsent stays held");
    };
    let record = store.load_attempt(&unsent).await.expect("load");
    assert_eq!(
        record.expect("row").resolved_by.as_deref(),
        Some(NOT_SENT_ACTOR)
    );
    store
        .retire_rejected(&second, &rejected, "refused", at(6))
        .await
        .expect("retire");
    assert_eq!(
        claim(&second, 1, 6).await,
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
