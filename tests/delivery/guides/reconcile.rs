//! Reconciliation under uncertainty: an unconfirmed create is found in the
//! channel or left unknown, never sent twice; unconfirmed deletes and edits
//! stop the run; a create cut by a shutdown is recovered on the next start.

use std::sync::Arc;

use kanade::bot::transport::{AmbiguousKind, Op, Step};
use kanade_domain::guides::{
    Blocked, Confirmations, GuideGrants, GuideStore, OpKind, OpOutcome, PlanOp, PublishKind,
    PublishOp, PublishState, SlotState, StopReason,
};
use kanade_domain::notify::{
    Claim, DeliveryJournal, EffectKind, IntentContent, NotificationIntent, Receipt,
};

use super::{ALL, CHANNEL, Rig, at, bosses, shown, text};

fn ambiguous(applied: bool) -> Step {
    Step::Ambiguous {
        kind: AmbiguousKind::Timeout,
        applied,
    }
}

const FOUND: u64 = 1_000_000_000_000_000_500;

#[tokio::test]
async fn an_ambiguous_create_found_once_in_the_channel_is_recovered_never_resent() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[bosses(1, None, false), text(2, "B")])
        .await;
    let desired = rig.desired().await;
    rig.queue(true).await;
    rig.fake.script(Op::Create, ambiguous(true));
    let hold = rig.fake.hold(Op::Create);
    // It lands after the channel check, as a real ambiguous post would.
    let landed = async {
        hold.entered().await;
        rig.fake.seed_history(vec![
            shown(FOUND, &desired[0], true),
            shown(FOUND + 1, &desired[0], false),
        ]);
        hold.release();
    };
    let (run, ()) = tokio::join!(rig.run(), landed);
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    let ops = rig.ops(&run).await;
    assert_eq!(ops[0].outcome, OpOutcome::Recovered);
    assert_eq!(ops[1].outcome, OpOutcome::Done, "the run went on");
    assert_eq!(rig.fake.count(Op::Create), 2, "never sent twice");
    let slots = rig.slots().await;
    assert_eq!(
        (slots[0].state, slots[0].message_id.clone()),
        (SlotState::Posted, Some(FOUND.to_string()))
    );
    assert_eq!(slots[0].uploads, desired[0].manifest());
}

#[tokio::test]
async fn an_ambiguous_create_not_found_is_unknown_and_nothing_later_is_sent() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    let desired = rig.desired().await;
    rig.queue(true).await;
    rig.fake.script(Op::Create, ambiguous(false));
    let run = rig.run().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::UnknownSlot))
    );
    assert_eq!(rig.fake.count(Op::Create), 1, "nothing later was sent");
    assert_eq!(
        rig.fake.count(Op::ChannelMessages),
        4,
        "the preview's and the worker's channel checks, then two searches"
    );
    let ops = rig.ops(&run).await;
    assert_eq!((ops.len(), ops[0].outcome), (1, OpOutcome::Unknown));
    let slot = &rig.slots().await[0];
    assert_eq!(
        (slot.state, slot.message_id.as_deref()),
        (SlotState::Unknown, None)
    );
    assert_eq!(slot.source, ops[0].source, "the journal source is kept");
    // The guide is blocked until an admin resolves it.
    assert!(matches!(
        rig.plan(true).await.blocked,
        Some(Blocked::UnknownSlot { position: 0, .. })
    ));

    // "Search again" (a run of kind search) finds it once it shows up.
    rig.fake.seed_history(vec![shown(FOUND, &desired[0], true)]);
    rig.request(
        PublishKind::Search,
        &"0".repeat(64),
        Confirmations::default(),
        false,
    )
    .await;
    let search = rig.run().await;
    assert_eq!(search.state, PublishState::Done, "{search:?}");
    let ops = rig.ops(&search).await;
    assert_eq!(
        (ops[0].op.kind(), ops[0].outcome),
        (OpKind::Create, OpOutcome::Recovered)
    );
    assert_eq!(rig.fake.count(Op::Create), 1, "a search sends nothing");
    let slot = &rig.slots().await[0];
    assert_eq!(slot.message_id.as_deref(), Some(FOUND.to_string().as_str()));
    assert_eq!(slot.state, SlotState::Posted);
}

#[tokio::test]
async fn two_matches_leave_the_slot_unknown_with_both_candidates() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "Same")]).await;
    let desired = rig.desired().await;
    rig.queue(true).await;
    rig.fake.script(Op::Create, ambiguous(true));
    let hold = rig.fake.hold(Op::Create);
    let landed = async {
        hold.entered().await;
        rig.fake.seed_history(vec![
            shown(FOUND, &desired[0], true),
            shown(FOUND + 1, &desired[0], true),
        ]);
        hold.release();
    };
    let (run, ()) = tokio::join!(rig.run(), landed);
    assert_eq!(run.stop_reason, Some(StopReason::UnknownSlot));
    let ops = rig.ops(&run).await;
    assert_eq!(ops[0].outcome, OpOutcome::Unknown);
    let mut candidates = ops[0].candidates.clone();
    candidates.sort();
    assert_eq!(candidates, [FOUND.to_string(), (FOUND + 1).to_string()]);
    assert_eq!(rig.slots().await[0].state, SlotState::Unknown);
}

#[tokio::test]
async fn an_ambiguous_delete_is_checked_and_unconfirmed_stops_before_any_create() {
    let rig = Rig::new().await;
    rig.save(
        CHANNEL,
        &[text(1, "A"), bosses(2, None, false), text(3, "C")],
    )
    .await;
    rig.publish().await;
    // A middle repost: deletes of 2 and 1, then creates.
    rig.save(
        CHANNEL,
        &[text(1, "A"), bosses(2, None, true), text(3, "C")],
    )
    .await;
    let creates = rig.fake.count(Op::Create);

    // Gone after all: the presence check confirms it, the run goes on.
    rig.fake.script(Op::Delete, ambiguous(true));
    let run = rig.publish().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    assert_eq!(rig.ops(&run).await[0].outcome, OpOutcome::Done);
    assert_eq!(rig.fake.count(Op::Create), creates + 2);

    // Still there after the retry too: unconfirmed, and no create follows.
    rig.save(
        CHANNEL,
        &[text(1, "A"), bosses(2, None, false), text(3, "C")],
    )
    .await;
    let before = rig.slots().await;
    rig.fake.script(Op::Delete, ambiguous(false));
    rig.fake.script(Op::Delete, ambiguous(false));
    let creates = rig.fake.count(Op::Create);
    let run = rig.publish().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::DeleteUnconfirmed))
    );
    assert_eq!(
        rig.fake.count(Op::Create),
        creates,
        "replace only after deletion"
    );
    assert_eq!(rig.ops(&run).await[0].outcome, OpOutcome::Unknown);
    assert_eq!(rig.slots().await, before, "the slot map is unchanged");
}

#[tokio::test]
async fn an_ambiguous_edit_is_retried_once() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A")]).await;
    rig.publish().await;

    rig.save(CHANNEL, &[text(1, "A2")]).await;
    rig.fake.script(Op::Edit, ambiguous(false));
    let run = rig.publish().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    assert_eq!(rig.fake.count(Op::Edit), 2);

    rig.save(CHANNEL, &[text(1, "A3")]).await;
    let before = rig.slots().await;
    rig.fake.script(Op::Edit, ambiguous(false));
    rig.fake.script(Op::Edit, ambiguous(true));
    let run = rig.publish().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::EditUnconfirmed))
    );
    assert_eq!(rig.fake.count(Op::Edit), 4, "one retry only");
    assert_eq!(rig.ops(&run).await[0].outcome, OpOutcome::Unknown);
    assert_eq!(
        rig.slots().await,
        before,
        "repeating an edit is safe: unchanged"
    );
}

#[tokio::test]
async fn a_create_cut_by_a_shutdown_is_recovered_on_the_next_start() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    let desired = rig.desired().await;
    let queued = rig.queue(true).await;
    let hold = rig.fake.hold(Op::Create);
    let publisher = Arc::clone(&rig.publisher);
    let stop = rig.stop.subscribe();
    let task = tokio::spawn(async move { publisher.run_next(&stop).await });
    hold.entered().await;
    // The process dies mid-create; the post had landed.
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    rig.fake.seed_history(vec![shown(FOUND, &desired[0], true)]);

    // Restart: the journal's recovery first, then the worker's.
    rig.store.recover_on_start(at()).await.unwrap();
    let restarted = Rig::with(
        Arc::clone(&rig.store),
        Arc::clone(&rig.fake),
        Arc::clone(&rig.channels),
    );
    restarted.publisher.recover().await;
    let (run, ops) = rig.store.publish(queued.id.clone()).await.unwrap().unwrap();
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::Interrupted))
    );
    assert_eq!(ops[0].outcome, OpOutcome::Recovered);
    assert_eq!(ops.len(), 1, "runs never resume on their own");
    let slot = &rig.slots().await[0];
    assert_eq!(slot.message_id.as_deref(), Some(FOUND.to_string().as_str()));
    assert_eq!(rig.fake.count(Op::Create), 0, "nothing was sent again");
}

#[tokio::test]
async fn a_create_bound_before_its_op_was_settled_is_recovered_from_the_journal() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A")]).await;
    let desired = rig.desired().await;
    let queued = rig.queue(true).await;
    // A process that posted and bound the create, then died before
    // settling the op.
    let run = rig.store.claim_next(at()).await.unwrap().unwrap();
    assert_eq!(run.id, queued.id);
    let source = "guide-post:00000000-0000-4000-a000-000000000001";
    rig.store
        .record_op(
            run.id.clone(),
            PublishOp {
                ordinal: 0,
                op: PlanOp::Create {
                    position: 0,
                    channel_id: CHANNEL.into(),
                    hash: desired[0].hash.clone(),
                },
                source: Some(source.into()),
                outcome: OpOutcome::Pending,
                candidates: Vec::new(),
                at: at(),
            },
        )
        .await
        .unwrap();
    let lease = rig
        .store
        .begin_lease("old", "guide_publish", at())
        .await
        .unwrap();
    let intent = NotificationIntent {
        effect: EffectKind::GuideSlot,
        effect_context: vec![run.id.clone(), "0".into()],
        channel_id: CHANNEL.into(),
        targets: Vec::new(),
        mentions: Vec::new(),
        content: IntentContent::Plain,
        warnings: Vec::new(),
    };
    let Claim::Fresh(attempt) = rig
        .store
        .claim_source(&lease, &intent, source, 0, at())
        .await
        .unwrap()
    else {
        panic!("claimed")
    };
    let receipt = Receipt {
        channel_id: CHANNEL.into(),
        message_id: FOUND.to_string(),
    };
    rig.store
        .bind(&lease, &attempt, &receipt, None, at())
        .await
        .unwrap();

    rig.store.recover_on_start(at()).await.unwrap();
    let reads = rig.fake.count(Op::ChannelMessages);
    rig.publisher.recover().await;
    let (run, ops) = rig.store.publish(run.id).await.unwrap().unwrap();
    assert_eq!(run.stop_reason, Some(StopReason::Interrupted));
    assert_eq!(ops[0].outcome, OpOutcome::Recovered);
    assert_eq!(
        rig.fake.count(Op::ChannelMessages),
        reads,
        "the journal's binding needs no search"
    );
    let slot = &rig.slots().await[0];
    assert_eq!(
        (slot.state, slot.message_id.as_deref()),
        (SlotState::Posted, Some(FOUND.to_string().as_str()))
    );
}

/// A search run re-reads the saved map and the channel before its first
/// Discord call: nothing left unknown sends nothing, and a channel the bot
/// can no longer read stops it before any search.
#[tokio::test]
async fn a_search_run_checks_the_saved_slots_and_the_channel_first() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A")]).await;
    rig.publish().await;
    let calls = rig.fake.calls().len();
    rig.request(
        PublishKind::Search,
        &"0".repeat(64),
        Confirmations::default(),
        false,
    )
    .await;
    let run = rig.run().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::PlanChanged))
    );
    assert_eq!(rig.fake.calls().len(), calls, "no Discord call at all");
    assert!(rig.ops(&run).await.is_empty());

    // An unknown slot, then the bot loses Read Message History.
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    rig.fake.script(Op::Create, ambiguous(false));
    assert_eq!(
        rig.publish().await.stop_reason,
        Some(StopReason::UnknownSlot)
    );
    rig.channels.set(
        CHANNEL,
        GuideGrants {
            history: false,
            ..ALL
        },
    );
    let reads = rig.fake.count(Op::ChannelMessages);
    rig.request(
        PublishKind::Search,
        &"0".repeat(64),
        Confirmations::default(),
        false,
    )
    .await;
    let run = rig.run().await;
    assert_eq!(run.stop_reason, Some(StopReason::Rejected));
    assert_eq!(
        rig.fake.count(Op::ChannelMessages),
        reads,
        "nothing searched"
    );
    assert_eq!(rig.slots().await[1].state, SlotState::Unknown);
}
