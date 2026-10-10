//! The proposal storage every store must keep: proposals are drafts of kind
//! `proposal` with their source, supersede key and TTL deadline, superseded
//! and expired as the system, and merged only through `commit_merge`. Each
//! check runs against a fresh store; failures panic with the check name.

use chrono::{DateTime, TimeDelta, TimeZone, Utc};

use crate::domain::drafts::{
    DEFAULT_PROPOSAL_TTL, DraftEventKind, DraftKind, DraftStatus, MergeCommit, NewDraft,
    NewProposal, ProposalCreated, ProposalSource, ProposalStore, ProposalSubmission, SUPERSEDED,
    StagedOp, Target,
};
use crate::domain::history::{Actor, ChangeHistory, ChangeMeta, ChangeRef, Origin, Surface};
use crate::domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, NotificationIntent, Receipt,
};
use crate::domain::proposals::{CardDetails, ChangeKind, ProposalCardStore, ProposalSubject};
use crate::domain::schedule::{Change, ChangeSet, Run, RunSource, RunStatus};
use crate::domain::scheduler::{CARDLESS_CHAT_GRACE, ScheduleStore, Scope, StoreError};

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<
    S: ScheduleStore + ChangeHistory + ProposalStore + ProposalCardStore + DeliveryJournal + Sync,
>(
    make: impl AsyncFn() -> S,
) {
    create_load_and_list(make().await).await;
    creation_refusals(make().await).await;
    same_source_ids_replay(make().await).await;
    newer_proposals_supersede_live_ones_with_their_key(make().await).await;
    ttl_expiry_closes_due_proposals_only(make().await).await;
    proposals_merge_through_commit_merge(make().await).await;
    cross_channel_lookup_preserves_existing_and_reads_bound_card(make().await).await;
    expired_proposals_do_not_block_lookup_or_create(make().await).await;
    concurrent_cross_channel_asks_create_only_one_proposal(make().await).await;
    cardless_chat_reuse_ends_at_the_grace_boundary(make().await).await;
    saved_chat_cards_outlive_the_cardless_grace(make().await).await;
    cardless_extraction_outlives_the_chat_grace(make().await).await;
}

fn utc(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, hour, minute, 0)
        .single()
        .expect("valid instant")
}

fn extractor() -> Actor {
    Actor::system("extraction")
}

async fn base<S: ScheduleStore + ChangeHistory>(store: &S) -> (ChangeRef, u64) {
    let head = store.history_head().await.expect("head");
    let revision = store.load(&Scope::All).await.expect("load").revision;
    (head, revision)
}

fn proposal(id: &str, base: &(ChangeRef, u64), key: Option<&str>) -> NewProposal {
    NewProposal {
        id: id.into(),
        title: "Lotus to Friday".into(),
        author: extractor(),
        base: base.0.clone(),
        base_revision: base.1,
        subject: Some("run:run-1".into()),
        at: utc(20, 12, 0),
        ops: vec![StagedOp {
            ord: 0,
            op: crate::domain::drafts::DraftOp::ResetToFixed {
                run: Target::Existing("run-1".into()),
            },
            author: extractor(),
            added_at: utc(20, 12, 0),
        }],
        expires_week: Some(utc(17, 0, 0)),
        source: ProposalSource::Extraction,
        source_id: format!("x-{id}"),
        supersede_key: key.map(str::to_owned),
        ttl: DEFAULT_PROPOSAL_TTL,
    }
}

async fn created<S: ProposalStore>(store: &S, new: NewProposal) -> Vec<String> {
    match store.create_proposal(new).await.expect("create") {
        ProposalCreated::Created { superseded, .. } => superseded,
        other => panic!("proposal not created: {other:?}"),
    }
}

async fn create_load_and_list<S: ScheduleStore + ChangeHistory + ProposalStore>(store: S) {
    let base = base(&store).await;
    let new = proposal("p-1", &base, None);
    let superseded = created(&store, new.clone()).await;
    assert!(superseded.is_empty());
    let (loaded, info) = store
        .load_proposal("p-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(loaded.draft.kind, DraftKind::Proposal, "create: kind");
    assert_eq!(loaded.draft.status, DraftStatus::Submitted);
    assert_eq!(loaded.draft.version, 1);
    assert_eq!(loaded.draft.author, extractor());
    assert_eq!(loaded.draft.subject.as_deref(), Some("run:run-1"));
    assert_eq!(loaded.draft.scope.expires_week(), Some(utc(17, 0, 0)));
    assert_eq!(
        loaded.draft_ops(),
        new.ops
            .iter()
            .map(|staged| staged.op.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(info.source, ProposalSource::Extraction);
    assert_eq!(info.source_id, "x-p-1");
    assert_eq!(
        info.expires_at,
        utc(21, 12, 0),
        "create: the deadline is created_at + TTL"
    );
    let plain = store
        .load_draft("p-1")
        .await
        .expect("load draft")
        .expect("present");
    assert_eq!(plain.draft.kind, DraftKind::Proposal, "drafts see the kind");
    let events = store.draft_events("p-1").await.expect("events");
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [DraftEventKind::Created, DraftEventKind::Submitted]
    );
    let listed = store.list_proposals(false).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].info, info);
    let drafts = store.list_drafts(None).await.expect("drafts");
    assert_eq!(
        drafts.iter().map(|draft| draft.kind).collect::<Vec<_>>(),
        [DraftKind::Proposal]
    );
    assert_eq!(store.load_proposal("absent").await.expect("load"), None);
}

async fn creation_refusals<S: ScheduleStore + ChangeHistory + ProposalStore>(store: S) {
    let base = base(&store).await;
    let mut member = proposal("p-1", &base, None);
    member.author = Actor::member("1");
    assert!(
        matches!(
            store.create_proposal(member).await,
            Err(StoreError::Constraint(_))
        ),
        "refusals: a proposal's author is a system component"
    );
    let mut zero = proposal("p-1", &base, None);
    zero.ttl = TimeDelta::zero();
    assert!(
        matches!(
            store.create_proposal(zero).await,
            Err(StoreError::Constraint(_))
        ),
        "refusals: the TTL is positive"
    );
    let plain = NewDraft {
        id: "p-2".into(),
        kind: DraftKind::Proposal,
        title: "sneaky".into(),
        author: extractor(),
        base: base.0.clone(),
        base_revision: base.1,
        request_type: None,
        subject: None,
        at: utc(20, 12, 0),
        request: None,
        submit: None,
    };
    assert!(
        matches!(
            store.create_draft(plain).await,
            Err(StoreError::Constraint(_))
        ),
        "refusals: create_draft never makes a proposal"
    );
    assert!(store.list_drafts(None).await.expect("drafts").is_empty());
}

async fn same_source_ids_replay<S: ScheduleStore + ChangeHistory + ProposalStore>(store: S) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, Some("run-1"))).await;
    let ProposalCreated::Replayed(replayed) = store
        .create_proposal(proposal("p-1", &base, Some("run-1")))
        .await
        .expect("replay")
    else {
        panic!("replay: not replayed");
    };
    assert_eq!(replayed.id, "p-1");
    let mut other = proposal("p-1", &base, None);
    other.source = ProposalSource::Chat;
    assert!(
        matches!(
            store.create_proposal(other).await,
            Err(StoreError::Constraint(_))
        ),
        "replay: another source's id is refused"
    );
    let admin = NewDraft {
        id: "d-1".into(),
        kind: DraftKind::Admin,
        title: "admin".into(),
        author: Actor::admin("root"),
        base: base.0.clone(),
        base_revision: base.1,
        request_type: None,
        subject: None,
        at: utc(20, 12, 0),
        request: None,
        submit: None,
    };
    store.create_draft(admin).await.expect("admin draft");
    assert!(
        matches!(
            store.create_proposal(proposal("d-1", &base, None)).await,
            Err(StoreError::Constraint(_))
        ),
        "replay: an admin draft's id is refused"
    );
    assert_eq!(
        store.list_proposals(false).await.expect("list").len(),
        1,
        "replay: nothing new written"
    );
}

async fn newer_proposals_supersede_live_ones_with_their_key<
    S: ScheduleStore + ChangeHistory + ProposalStore,
>(
    store: S,
) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, Some("run-1"))).await;
    created(&store, proposal("p-2", &base, Some("run-2"))).await;
    created(&store, proposal("p-3", &base, None)).await;
    let mut newer = proposal("p-4", &base, Some("run-1"));
    newer.at = utc(20, 13, 0);
    assert_eq!(created(&store, newer).await, ["p-1"], "supersede: same key");
    let (old, _) = store
        .load_proposal("p-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(old.draft.status, DraftStatus::Discarded);
    assert_eq!(old.draft.close_reason.as_deref(), Some(SUPERSEDED));
    assert_eq!(
        old.draft.closed_by,
        Some(extractor()),
        "supersede: closed by the system author"
    );
    assert_eq!(old.draft.updated_at, utc(20, 13, 0));
    assert_eq!(old.draft.version, 1, "supersede: no version bump");
    let events = store.draft_events("p-1").await.expect("events");
    let last = events.last().expect("event");
    assert_eq!(last.kind, DraftEventKind::Discarded);
    assert_eq!(last.detail.as_deref(), Some(SUPERSEDED));
    let mut newest = proposal("p-5", &base, Some("run-1"));
    newest.at = utc(20, 14, 0);
    assert_eq!(
        created(&store, newest).await,
        ["p-4"],
        "supersede: closed proposals are left alone"
    );
    let live = store.list_proposals(true).await.expect("live");
    assert_eq!(
        live.iter()
            .map(|proposal| proposal.draft.id.as_str())
            .collect::<Vec<_>>(),
        ["p-2", "p-3", "p-5"]
    );
}

async fn ttl_expiry_closes_due_proposals_only<S: ScheduleStore + ChangeHistory + ProposalStore>(
    store: S,
) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, None)).await;
    let mut short = proposal("p-2", &base, None);
    short.ttl = TimeDelta::hours(2);
    created(&store, short).await;
    let delivery = Actor::system("delivery");
    assert!(
        store
            .expire_proposals(utc(20, 13, 59), &delivery)
            .await
            .expect("early")
            .is_empty()
    );
    assert_eq!(
        store
            .expire_proposals(utc(20, 14, 0), &delivery)
            .await
            .expect("due"),
        ["p-2"],
        "ttl: due at the deadline"
    );
    let (expired, _) = store
        .load_proposal("p-2")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(expired.draft.status, DraftStatus::Expired);
    assert_eq!(expired.draft.closed_by, Some(delivery.clone()));
    assert_eq!(expired.draft.version, 1, "ttl: no version bump");
    assert_eq!(
        store
            .expire_proposals(utc(21, 12, 0), &delivery)
            .await
            .expect("later"),
        ["p-1"],
        "ttl: expired proposals are not expired again"
    );
    assert!(store.list_proposals(true).await.expect("live").is_empty());
}

async fn proposals_merge_through_commit_merge<S: ScheduleStore + ChangeHistory + ProposalStore>(
    store: S,
) {
    let base = base(&store).await;
    created(&store, proposal("p-1", &base, None)).await;
    let week = utc(14, 16, 0);
    let run = Run {
        id: "run-9".into(),
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start: week,
        datetime: utc(18, 20, 0),
        bosses: vec!["HFA".into()],
        participants: vec!["1".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    };
    let meta = ChangeMeta {
        origin: Origin::new(Actor::admin("root"), Surface::ExtractionApproval)
            .with_request_id("merge:p-1@v1"),
        at: utc(20, 15, 0),
        notices: Vec::new(),
        refs: vec![base.0.clone()],
        request_digest: Some("digest".into()),
        expect: Default::default(),
        outbox: Vec::new(),
    };
    let MergeCommit::Committed(committed) = store
        .commit_merge(
            base.1,
            ChangeSet {
                changes: vec![Change::PutRun(run)],
            },
            meta,
            "p-1",
            1,
            None,
        )
        .await
        .expect("merge")
    else {
        panic!("merge: not committed");
    };
    let (merged, _) = store
        .load_proposal("p-1")
        .await
        .expect("load")
        .expect("present");
    assert_eq!(merged.draft.status, DraftStatus::Merged);
    assert_eq!(merged.draft.merged_seq, Some(committed.seq));
    assert_eq!(merged.draft.kind, DraftKind::Proposal);
    let record = store
        .load_change(committed.seq)
        .await
        .expect("record")
        .expect("record");
    assert_eq!(record.origin.surface, Surface::ExtractionApproval);
    assert!(
        store
            .expire_proposals(utc(30, 0, 0), &Actor::system("delivery"))
            .await
            .expect("expire")
            .is_empty(),
        "merge: a merged proposal never expires"
    );
}

fn run_move(
    id: &str,
    base: &(ChangeRef, u64),
    channel: &str,
    source: ProposalSource,
) -> NewProposal {
    let mut new = proposal(id, base, None);
    new.source = source;
    new.author = Actor::system(source.as_str());
    new.subject = Some(
        ProposalSubject {
            kind: ChangeKind::Move,
            run_id: Some("run-1".into()),
            fixed_run_id: None,
            channel_id: Some(channel.into()),
            bosses: vec!["HFA".into()],
            named: Vec::new(),
        }
        .encode(),
    );
    new.ops[0].author = new.author.clone();
    new.ops[0].op = crate::domain::drafts::DraftOp::AmendRun {
        run: Target::Existing("run-1".into()),
        to: utc(21, 22, 0),
    };
    new
}

async fn cross_channel_lookup_preserves_existing_and_reads_bound_card<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore + ProposalCardStore + DeliveryJournal,
{
    let base = base(&store).await;
    let original = run_move("p-old", &base, "900", ProposalSource::Extraction);
    created(&store, original).await;
    let new = run_move("p-new", &base, "901", ProposalSource::Chat);
    let before = store.load_proposal("p-old").await.unwrap();
    let events = store.draft_events("p-old").await.unwrap();
    let week = utc(17, 0, 0);
    for stage in 0..3 {
        if stage == 1 {
            let details =
                CardDetails::from_json(&serde_json::json!({"kind": "move", "run_id": "run-1"}))
                    .unwrap();
            store
                .save_card("p-old", "900", &details, new.at)
                .await
                .unwrap();
        }
        if stage == 2 {
            let lease = store
                .begin_lease("test", "lookup-bind", new.at)
                .await
                .unwrap();
            let intent = NotificationIntent {
                effect: EffectKind::Card,
                effect_context: Vec::new(),
                channel_id: "900".into(),
                targets: vec![DeliveryTarget::Card("p-old".into())],
                mentions: Vec::new(),
                warnings: Vec::new(),
                content: IntentContent::ProposalCard {
                    proposal_ids: vec!["p-old".into()],
                },
            };
            let Claim::Fresh(attempt) = store.claim(&lease, &intent, None, new.at).await.unwrap()
            else {
                panic!("fresh claim")
            };
            store
                .bind(
                    &lease,
                    &attempt,
                    &Receipt {
                        channel_id: "900".into(),
                        message_id: "9500".into(),
                    },
                    None,
                    new.at,
                )
                .await
                .unwrap();
            store.end_lease(&lease, new.at).await.unwrap();
        }
        let cards = store.load_cards(&["p-old".into()]).await.unwrap();
        let ProposalSubmission::Existing(existing) = store
            .create_proposal_or_existing(new.clone(), week)
            .await
            .unwrap()
        else {
            panic!("existing cross-channel move")
        };
        assert_eq!(existing.proposal_id, "p-old");
        assert_eq!(existing.channel_id, "900");
        assert_eq!(
            existing.message_id.as_deref(),
            (stage == 2).then_some("9500")
        );
        assert_eq!(store.list_proposals(false).await.unwrap().len(), 1);
        assert_eq!(store.load_proposal("p-old").await.unwrap(), before);
        assert_eq!(store.draft_events("p-old").await.unwrap(), events);
        assert_eq!(store.load_cards(&["p-old".into()]).await.unwrap(), cards);
        assert!(store.draft_events("p-new").await.unwrap().is_empty());
    }
    // Same id still replays, even when a matching sibling could be reused.
    let replay = run_move("p-old", &base, "900", ProposalSource::Extraction);
    assert!(
        matches!(
            store.create_proposal_or_existing(replay, week).await,
            Err(StoreError::Constraint(_))
        ),
        "chat-only port"
    );
    let chat = run_move("p-chat", &base, "901", ProposalSource::Chat);
    created(&store, chat.clone()).await;
    assert!(
        matches!(store.create_proposal_or_existing(chat, week).await.unwrap(), ProposalSubmission::Created(created) if matches!(*created, ProposalCreated::Replayed(_)))
    );
    let mut conflicting = new;
    conflicting.id = "p-old".into();
    assert!(matches!(
        store.create_proposal_or_existing(conflicting, week).await,
        Err(StoreError::Constraint(_))
    ));
}

async fn expired_proposals_do_not_block_lookup_or_create<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore,
{
    let base = base(&store).await;
    let mut ttl = run_move("p-ttl", &base, "900", ProposalSource::Extraction);
    ttl.ttl = TimeDelta::hours(1);
    created(&store, ttl.clone()).await;
    let mut past_week = run_move("p-week", &base, "900", ProposalSource::Chat);
    past_week.expires_week = Some(utc(10, 0, 0));
    created(&store, past_week).await;
    let mut new = run_move("p-new", &base, "901", ProposalSource::Chat);
    new.at = ttl.at + ttl.ttl;
    assert!(matches!(
        store
            .create_proposal_or_existing(new, utc(17, 0, 0))
            .await
            .unwrap(),
        ProposalSubmission::Created(_)
    ));
    assert_eq!(
        store
            .load_proposal("p-ttl")
            .await
            .unwrap()
            .unwrap()
            .0
            .draft
            .status,
        DraftStatus::Submitted,
        "lookup never closes expired rows"
    );
    assert_eq!(store.list_proposals(false).await.unwrap().len(), 3);
}

async fn concurrent_cross_channel_asks_create_only_one_proposal<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore + Sync,
{
    let base = base(&store).await;
    let first = run_move("p-a", &base, "900", ProposalSource::Chat);
    let second = run_move("p-b", &base, "901", ProposalSource::Chat);
    let barrier = tokio::sync::Barrier::new(2);
    let week = utc(17, 0, 0);
    let (a, b) = tokio::join!(
        async {
            barrier.wait().await;
            store
                .create_proposal_or_existing(first, week)
                .await
                .unwrap()
        },
        async {
            barrier.wait().await;
            store
                .create_proposal_or_existing(second, week)
                .await
                .unwrap()
        },
    );
    let (created, existing) = match (a, b) {
        (ProposalSubmission::Created(created), ProposalSubmission::Existing(existing))
        | (ProposalSubmission::Existing(existing), ProposalSubmission::Created(created)) => {
            (created, existing)
        }
        other => panic!("one creation and one reuse: {other:?}"),
    };
    let ProposalCreated::Created { draft, superseded } = *created else {
        panic!("new proposal")
    };
    assert!(superseded.is_empty());
    assert_eq!(existing.proposal_id, draft.id);
    assert_eq!(existing.message_id, None);
    assert_eq!(store.list_proposals(false).await.unwrap().len(), 1);
    assert_eq!(store.list_drafts(None).await.unwrap().len(), 1);
}

async fn cardless_chat_reuse_ends_at_the_grace_boundary<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore,
{
    let base = base(&store).await;
    let old = run_move("p-old", &base, "900", ProposalSource::Chat);
    created(&store, old.clone()).await;
    let before = store.load_proposal("p-old").await.unwrap();
    let mut new = run_move("p-new", &base, "901", ProposalSource::Chat);
    new.at = old.at + CARDLESS_CHAT_GRACE - TimeDelta::seconds(1);
    assert!(
        matches!(
            store
                .create_proposal_or_existing(new.clone(), utc(17, 0, 0))
                .await
                .unwrap(),
            ProposalSubmission::Existing(_)
        ),
        "in-flight grace"
    );
    new.at = old.at + CARDLESS_CHAT_GRACE;
    assert!(
        matches!(
            store
                .create_proposal_or_existing(new, utc(17, 0, 0))
                .await
                .unwrap(),
            ProposalSubmission::Created(_)
        ),
        "expired grace, even at its exact boundary"
    );
    assert_eq!(
        store.load_proposal("p-old").await.unwrap(),
        before,
        "cardless draft remains actionable in the admin Inbox"
    );
    assert_eq!(store.list_proposals(false).await.unwrap().len(), 2);
}

async fn saved_chat_cards_outlive_the_cardless_grace<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore + ProposalCardStore,
{
    let base = base(&store).await;
    let old = run_move("p-old", &base, "900", ProposalSource::Chat);
    created(&store, old.clone()).await;
    let details =
        CardDetails::from_json(&serde_json::json!({"kind": "move", "run_id": "run-1"})).unwrap();
    store
        .save_card("p-old", "900", &details, old.at)
        .await
        .unwrap();
    let mut new = run_move("p-new", &base, "901", ProposalSource::Chat);
    new.at = old.at + CARDLESS_CHAT_GRACE;
    assert!(
        matches!(store.create_proposal_or_existing(new, utc(17, 0, 0)).await.unwrap(), ProposalSubmission::Existing(existing) if existing.message_id.is_none()),
        "saved details, unbound card remains eligible"
    );
    assert_eq!(store.list_proposals(false).await.unwrap().len(), 1);
}

async fn cardless_extraction_outlives_the_chat_grace<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore,
{
    let base = base(&store).await;
    let old = run_move("p-old", &base, "900", ProposalSource::Extraction);
    created(&store, old.clone()).await;
    let mut new = run_move("p-new", &base, "901", ProposalSource::Chat);
    new.at = old.at + CARDLESS_CHAT_GRACE;
    assert!(
        matches!(
            store
                .create_proposal_or_existing(new, utc(17, 0, 0))
                .await
                .unwrap(),
            ProposalSubmission::Existing(_)
        ),
        "grace applies only to chat without saved card details"
    );
    assert_eq!(store.list_proposals(false).await.unwrap().len(), 1);
}
