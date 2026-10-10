//! Proposal cards every store must keep: details written once beside a
//! proposal, bound to their Discord message only by the journal's `card`
//! target, and never claimed again once posted, closed or retired unproven.

use chrono::{DateTime, TimeZone, Utc};

use crate::domain::drafts::{
    DEFAULT_PROPOSAL_TTL, DraftChange, DraftStatus, DraftUpdate, NewProposal, ProposalSource,
    ProposalStore, StagedOp, Target,
};
use crate::domain::history::{Actor, ChangeHistory};
use crate::domain::notify::{
    Claim, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent, JournalError, Lease,
    NotificationIntent, Receipt,
};
use crate::domain::proposals::{CardDetails, CardPayload, ChangeKind, ProposalCardStore};
use crate::domain::scheduler::{ScheduleStore, Scope, StoreError};

const CHANNEL: &str = "900";

/// Run every check, each against a fresh store from `make`.
pub async fn run_suite<S>(make: impl AsyncFn() -> S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore + ProposalCardStore + DeliveryJournal + Sync,
{
    details_are_written_once_beside_a_proposal(make().await).await;
    the_journal_binds_a_card_once(make().await).await;
}

fn utc(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 20, hour, 0, 0)
        .single()
        .expect("valid instant")
}

fn details(summary: &str) -> CardDetails {
    CardDetails {
        kind: ChangeKind::Move,
        run_id: Some("run-1".into()),
        bosses: vec!["HMaleficStar".into()],
        participants: vec!["1".into()],
        new_datetime: Some(utc(13)),
        day_ref: Some("wed".into()),
        time_ref: None,
        rsvp: None,
        is_question: false,
        summary: Some(summary.into()),
        also_mentioned: vec!["otot".into()],
        confidence: 0.9,
        payload: CardPayload::default(),
        evidence_message_ids: vec!["101".into()],
        self_service: None,
    }
}

async fn propose<S: ScheduleStore + ChangeHistory + ProposalStore>(store: &S, id: &str) {
    let head = store.history_head().await.expect("head");
    let revision = store.load(&Scope::All).await.expect("load").revision;
    let author = Actor::system("extraction");
    store
        .create_proposal(NewProposal {
            id: id.into(),
            title: "move proposal".into(),
            author: author.clone(),
            base: head,
            base_revision: revision,
            subject: None,
            at: utc(12),
            ops: vec![StagedOp {
                ord: 0,
                op: crate::domain::drafts::DraftOp::ResetToFixed {
                    run: Target::Existing("run-1".into()),
                },
                author,
                added_at: utc(12),
            }],
            expires_week: None,
            source: ProposalSource::Extraction,
            source_id: format!("x-{id}"),
            supersede_key: None,
            ttl: DEFAULT_PROPOSAL_TTL,
        })
        .await
        .expect("proposal");
}

async fn details_are_written_once_beside_a_proposal<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore + ProposalCardStore,
{
    assert!(
        matches!(
            store
                .save_card("absent", CHANNEL, &details("x"), utc(12))
                .await,
            Err(StoreError::Constraint(_))
        ),
        "cards: only a proposal has a card"
    );
    propose(&store, "p-1").await;
    propose(&store, "p-2").await;
    store
        .save_card("p-2", CHANNEL, &details("later"), utc(13))
        .await
        .expect("save");
    store
        .save_card("p-1", CHANNEL, &details("first"), utc(12))
        .await
        .expect("save");
    store
        .save_card("p-1", CHANNEL, &details("first"), utc(14))
        .await
        .expect("the same card again is a no-op");
    assert!(
        matches!(
            store
                .save_card("p-1", CHANNEL, &details("other"), utc(14))
                .await,
            Err(StoreError::Constraint(_))
        ),
        "cards: details never change"
    );
    let loaded = store
        .load_cards(&["p-2".into(), "absent".into(), "p-1".into()])
        .await
        .expect("load");
    let ids: Vec<&str> = loaded
        .iter()
        .map(|card| card.proposal_id.as_str())
        .collect();
    assert_eq!(ids, ["p-2", "p-1"], "cards: in the asked order");
    assert_eq!(loaded[1].details, details("first"));
    assert_eq!(loaded[1].message_id, None);
    let unposted = store.unposted_cards(CHANNEL).await.expect("unposted");
    let ids: Vec<&str> = unposted
        .iter()
        .map(|card| card.proposal_id.as_str())
        .collect();
    assert_eq!(ids, ["p-1", "p-2"], "cards: unposted, oldest first");
    assert!(store.unposted_cards("901").await.expect("other").is_empty());
}

fn intent(ids: &[&str]) -> NotificationIntent {
    NotificationIntent {
        effect: EffectKind::Card,
        effect_context: Vec::new(),
        channel_id: CHANNEL.into(),
        targets: ids
            .iter()
            .map(|id| DeliveryTarget::Card((*id).into()))
            .collect(),
        mentions: Vec::new(),
        content: IntentContent::ProposalCard {
            proposal_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        },
        warnings: Vec::new(),
    }
}

async fn fresh<S: DeliveryJournal>(store: &S, lease: &Lease, ids: &[&str]) -> Claim {
    store
        .claim(lease, &intent(ids), None, utc(15))
        .await
        .expect("claim")
}

async fn the_journal_binds_a_card_once<S>(store: S)
where
    S: ScheduleStore + ChangeHistory + ProposalStore + ProposalCardStore + DeliveryJournal,
{
    for id in ["p-1", "p-2", "p-3"] {
        propose(&store, id).await;
        store
            .save_card(id, CHANNEL, &details(id), utc(12))
            .await
            .expect("save");
    }
    let lease = store
        .begin_lease("instance-1", "card", utc(15))
        .await
        .expect("lease");
    assert!(
        matches!(
            store
                .claim(&lease, &intent(&["absent"]), None, utc(15))
                .await,
            Err(JournalError::TargetUnavailable(_))
        ),
        "cards: no card, no claim"
    );
    let Claim::Fresh(attempt) = fresh(&store, &lease, &["p-1", "p-2"]).await else {
        panic!("cards: a fresh claim");
    };
    assert_eq!(
        fresh(&store, &lease, &["p-2"]).await,
        Claim::Held,
        "cards: a claimed card is held"
    );
    let receipt = Receipt {
        channel_id: CHANNEL.into(),
        message_id: "5000".into(),
    };
    store
        .bind(&lease, &attempt, &receipt, None, utc(15))
        .await
        .expect("bind");
    let posted = store.cards_on_message("5000").await.expect("lookup");
    let ids: Vec<&str> = posted
        .iter()
        .map(|card| card.proposal_id.as_str())
        .collect();
    assert_eq!(ids, ["p-1", "p-2"], "cards: bound to the message");
    assert_eq!(posted[0].posted_at, Some(utc(15)));
    assert!(
        matches!(
            store.claim(&lease, &intent(&["p-1"]), None, utc(15)).await,
            Ok(Claim::Held) | Err(JournalError::TargetUnavailable(_))
        ),
        "cards: a posted card is never claimed again"
    );
    let ids: Vec<String> = store
        .unposted_cards(CHANNEL)
        .await
        .expect("unposted")
        .into_iter()
        .map(|card| card.proposal_id)
        .collect();
    assert_eq!(ids, ["p-3"]);

    let draft = store.load_draft("p-3").await.expect("load").expect("draft");
    store
        .update_draft(DraftUpdate {
            draft_id: "p-3".into(),
            expected_version: draft.draft.version,
            actor: Actor::member("1"),
            at: utc(15),
            change: DraftChange::Close {
                status: DraftStatus::Rejected,
                reason: None,
                notices: Vec::new(),
            },
        })
        .await
        .expect("close");
    assert!(
        matches!(
            store.claim(&lease, &intent(&["p-3"]), None, utc(15)).await,
            Err(JournalError::TargetUnavailable(_))
        ),
        "cards: a closed proposal's card is not posted"
    );
    assert!(
        store
            .unposted_cards(CHANNEL)
            .await
            .expect("unposted")
            .is_empty()
    );
}
