//! Confirmed plans run in order, silently, mentioning nobody, uploading only
//! on create; anything not confirmed (a moved plan, a missing grant, a
//! repost without its confirmation) sends nothing.

use std::time::Duration;

use tokio::time::Instant;
use twilight_model::id::Id;

use kanade::bot::mentions;
use kanade::bot::transport::{
    AmbiguousKind, Call, DiscordTransport, Op, RejectionKind, SILENT, Step,
};
use kanade_domain::guides::{
    Blocked, Confirmations, Grant, GuideGrants, GuideStore, OpKind, OpOutcome, PublishKind,
    PublishState, SlotState, StopReason,
};

use super::{ALL, CHANNEL, GUIDE, OLD, Rig, bosses, labels, spacer, text};

fn outcomes(ops: &[kanade_domain::guides::PublishOp]) -> Vec<(OpKind, u32, OpOutcome)> {
    ops.iter()
        .map(|op| (op.op.kind(), op.op.position(), op.outcome))
        .collect()
}

#[tokio::test]
async fn a_first_publish_creates_every_slot_in_order_silently_and_a_no_op_sends_nothing() {
    let rig = Rig::new().await;
    rig.save(
        CHANNEL,
        &[
            text(1, "Intro"),
            spacer(2),
            bosses(3, None, false),
            text(4, "Outro"),
        ],
    )
    .await;
    let run = rig.publish().await;
    assert_eq!((run.state, run.stop_reason), (PublishState::Done, None));
    assert_eq!(
        labels(&rig.effects()),
        [
            format!("create:{CHANNEL}:Intro"),
            format!("create:{CHANNEL}:\u{200B}"),
            format!("create:{CHANNEL}:Invented heading"),
            format!("create:{CHANNEL}:Outro"),
        ]
    );
    assert_eq!(
        rig.fake.create_flags(),
        vec![SILENT; 4],
        "every create is silent"
    );
    for call in rig.effects() {
        let Call::Create { message, .. } = call else {
            unreachable!()
        };
        assert_eq!(message.allowed_mentions, mentions::none());
        let expected = usize::from(message.content.as_deref() == Some("Invented heading"));
        assert_eq!(
            message.attachments.len(),
            expected,
            "art goes with its slot"
        );
    }
    let slots = rig.slots().await;
    let ids: Vec<_> = slots
        .iter()
        .map(|slot| (slot.position, slot.state, slot.message_id.is_some()))
        .collect();
    assert_eq!(
        ids,
        (0..4)
            .map(|n| (n, SlotState::Posted, true))
            .collect::<Vec<_>>()
    );
    assert!(
        outcomes(&rig.ops(&run).await)
            .iter()
            .all(|(kind, _, outcome)| *kind == OpKind::Create && *outcome == OpOutcome::Done)
    );

    let plan = rig.plan(true).await;
    assert!(plan.no_op() && plan.checked, "{plan:?}");
    let before = rig.effect_count();
    let run = rig.publish().await;
    assert_eq!(run.state, PublishState::Done);
    assert_eq!(
        rig.effect_count(),
        before,
        "a no-op makes no Discord effect"
    );
}

#[tokio::test]
async fn a_changed_slot_is_edited_in_place_keeping_its_files_and_the_tail_appended() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "Intro"), bosses(2, None, false)])
        .await;
    rig.publish().await;
    let posted = rig.slots().await;
    rig.save(
        CHANNEL,
        &[
            text(1, "Intro, revised"),
            bosses(2, Some("Invented note"), false),
            text(3, "New tail"),
        ],
    )
    .await;
    let before = rig.effect_count();
    let run = rig.publish().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    let effects = rig.effects().split_off(before);
    let edited: Vec<_> = posted
        .iter()
        .map(|slot| format!("edit:{}", slot.message_id.as_deref().unwrap()))
        .collect();
    assert_eq!(
        labels(&effects),
        [
            edited[0].clone(),
            edited[1].clone(),
            format!("create:{CHANNEL}:New tail"),
        ]
    );
    let Call::Edit { edit, .. } = &effects[1] else {
        unreachable!()
    };
    assert_eq!(edit.allowed_mentions, mentions::none());
    let embed = &edit.embeds.as_ref().unwrap()[0];
    assert!(
        embed
            .description
            .as_deref()
            .unwrap()
            .contains("Invented note")
    );
    assert_eq!(
        embed.thumbnail.as_ref().unwrap().url,
        "attachment://MaleficStar-portrait.png",
        "the edit refers to the file posted with the message, sending none"
    );
    let slots = rig.slots().await;
    assert_eq!(slots[1].message_id, posted[1].message_id, "edited in place");
    assert_eq!(slots[1].uploads, posted[1].uploads);

    // Removing the tail deletes it, newest first.
    rig.save(CHANNEL, &[text(1, "Intro, revised")]).await;
    let before = rig.effect_count();
    rig.publish().await;
    let slots = rig.slots().await;
    let deleted: Vec<_> = labels(&rig.effects().split_off(before));
    assert_eq!(deleted.len(), 2, "{deleted:?}");
    assert!(deleted.iter().all(|label| label.starts_with("delete:")));
    assert!(deleted[0] > deleted[1], "newest first: {deleted:?}");
    assert_eq!(slots.len(), 1);
}

#[tokio::test]
async fn a_middle_repost_runs_only_with_its_confirmation_deleting_before_creating() {
    let rig = Rig::new().await;
    rig.save(
        CHANNEL,
        &[text(1, "A"), bosses(2, None, false), text(3, "C")],
    )
    .await;
    rig.publish().await;
    let posted = rig.slots().await;
    // The entry art adds a file: the middle message cannot be edited.
    rig.save(
        CHANNEL,
        &[text(1, "A"), bosses(2, None, true), text(3, "C")],
    )
    .await;
    let plan = rig.plan(true).await;
    assert_eq!(plan.needs.repost_from, Some(1), "{plan:?}");

    let before = rig.effect_count();
    rig.request(
        PublishKind::Publish,
        &plan.hash,
        Confirmations::default(),
        true,
    )
    .await;
    let refused = rig.run().await;
    assert_eq!(
        (refused.state, refused.stop_reason),
        (PublishState::Stopped, Some(StopReason::PlanChanged))
    );
    assert_eq!(rig.effect_count(), before, "unconfirmed: nothing sent");
    assert_eq!(rig.slots().await, posted);

    rig.request(PublishKind::Publish, &plan.hash, plan.needs.clone(), true)
        .await;
    let run = rig.run().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    let message = |at: usize| posted[at].message_id.clone().unwrap();
    let effects = rig.effects().split_off(before);
    let labels = labels(&effects);
    assert_eq!(
        labels[..2],
        [
            format!("delete:{CHANNEL}:{}", message(2)),
            format!("delete:{CHANNEL}:{}", message(1)),
        ]
    );
    assert_eq!(labels[2], format!("create:{CHANNEL}:Invented heading"));
    assert_eq!(labels[3], format!("create:{CHANNEL}:C"));
    let Call::Create {
        message: created, ..
    } = &effects[2]
    else {
        unreachable!()
    };
    assert_eq!(
        created.attachments.len(),
        2,
        "both pictures uploaded on create"
    );
}

#[tokio::test]
async fn an_edit_finding_its_message_gone_stops_the_run_and_the_next_plan_asks() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B"), text(3, "C")])
        .await;
    rig.publish().await;
    rig.save(CHANNEL, &[text(1, "A2"), text(2, "B2"), text(3, "C")])
        .await;
    rig.queue(true).await;
    rig.fake
        .script(Op::Edit, Step::Reject(RejectionKind::UnknownMessage));
    let run = rig.run().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::PlanChanged))
    );
    assert_eq!(
        outcomes(&rig.ops(&run).await),
        [(OpKind::Edit, 0, OpOutcome::Gone)],
        "nothing after the 404"
    );
    let slots = rig.slots().await;
    assert_eq!(slots[0].state, SlotState::Gone);
    assert!(slots[0].message_id.is_some(), "the gone message id is kept");
    // A gone middle message is a middle repost (decision 10).
    assert_eq!(rig.plan(true).await.needs.repost_from, Some(0));
}

#[tokio::test]
async fn a_channel_move_posts_the_new_set_before_deleting_the_old_one() {
    let rig = Rig::new().await;
    rig.save(OLD, &[text(1, "A"), text(2, "B")]).await;
    rig.publish().await;
    let old: Vec<_> = rig.slots().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    let plan = rig.plan(true).await;
    assert!(plan.needs.channel_move);

    // A create Discord refuses stops the move: the old set stays.
    rig.fake
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let before = rig.effect_count();
    rig.queue(true).await;
    let refused = rig.run().await;
    assert_eq!(refused.stop_reason, Some(StopReason::Rejected));
    assert_eq!(
        labels(&rig.effects().split_off(before)),
        [format!("create:{CHANNEL}:A")]
    );
    assert_eq!(
        rig.slots().await,
        old,
        "no delete before every create is bound"
    );

    let before = rig.effect_count();
    let run = rig.publish().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    let message = |at: usize| old[at].message_id.clone().unwrap();
    assert_eq!(
        labels(&rig.effects().split_off(before)),
        [
            format!("create:{CHANNEL}:A"),
            format!("create:{CHANNEL}:B"),
            format!("delete:{OLD}:{}", message(1)),
            format!("delete:{OLD}:{}", message(0)),
        ]
    );
    let slots = rig.slots().await;
    assert!(slots.iter().all(|slot| slot.channel_id == CHANNEL));
    assert_eq!(slots.len(), 2);
}

#[tokio::test]
async fn a_cancel_ends_the_run_between_ops() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B"), text(3, "C")])
        .await;
    let queued = rig.queue(true).await;
    let hold = rig.fake.hold(Op::Create);
    let cancel = async {
        hold.entered().await;
        let asked = rig
            .store
            .request_cancel(GUIDE.into(), queued.id.clone(), super::at())
            .await
            .unwrap();
        assert!(matches!(
            asked,
            kanade_domain::guides::CancelRequest::Requested(_)
        ));
        hold.release();
    };
    let (run, ()) = tokio::join!(rig.run(), cancel);
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Cancelled, None)
    );
    assert_eq!(
        rig.fake.count(Op::Create),
        1,
        "the op in flight finishes, no more"
    );
    assert_eq!(
        outcomes(&rig.ops(&run).await),
        [(OpKind::Create, 0, OpOutcome::Done)]
    );
    assert_eq!(rig.slots().await.len(), 1, "the slot map stays exact");
}

#[tokio::test]
async fn a_moved_plan_or_a_missing_grant_sends_nothing() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), bosses(2, None, false)])
        .await;
    rig.request(
        PublishKind::Publish,
        &"0".repeat(64),
        Confirmations::default(),
        true,
    )
    .await;
    let run = rig.run().await;
    assert_eq!(run.stop_reason, Some(StopReason::PlanChanged));

    // Saved after the admin confirmed: the worker re-plans from the guide.
    let queued = rig.queue(true).await;
    rig.save(CHANNEL, &[text(1, "A, edited"), bosses(2, None, false)])
        .await;
    assert_eq!(rig.run().await.id, queued.id);
    assert_eq!(rig.effect_count(), 0);

    rig.channels.set(
        CHANNEL,
        GuideGrants {
            attach: false,
            history: false,
            ..ALL
        },
    );
    let plan = rig.plan(true).await;
    assert_eq!(
        plan.blocked,
        Some(Blocked::Grants(vec![Grant::History, Grant::Attach]))
    );
    rig.request(PublishKind::Publish, &plan.hash, plan.needs, true)
        .await;
    let run = rig.run().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::Rejected))
    );
    assert_eq!(rig.effect_count(), 0, "blocked before any Discord effect");
    assert!(rig.ops(&run).await.is_empty());
}

#[tokio::test]
async fn an_unpublish_deletes_every_message_newest_first() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    rig.publish().await;
    rig.fake.script(
        Op::Delete,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
    );
    rig.queue_unpublish().await;
    let run = rig.run().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    assert!(rig.slots().await.is_empty());
    assert!(rig.fake.messages().is_empty());
    let after_first_delete = rig
        .fake
        .calls()
        .into_iter()
        .skip_while(|call| call.op() != Op::Delete)
        .filter(|call| call.op() == Op::Presence)
        .count();
    assert_eq!(after_first_delete, 1, "the ambiguous delete was checked");
}

/// An unpublish is planned from the stored map; a message deleted by hand
/// before it runs is found by the run's channel check and only dropped
/// from the map, never deleted again (no 404 delete).
#[tokio::test]
async fn an_unpublish_drops_a_message_deleted_by_hand_without_deleting_it() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    rig.publish().await;
    let slots = rig.slots().await;
    let id = |at: usize| slots[at].message_id.clone().unwrap();
    rig.queue_unpublish().await;
    // Deleted by hand after the admin confirmed.
    let hand = rig
        .fake
        .delete_message(
            Id::new(CHANNEL.parse().unwrap()),
            Id::new(id(0).parse().unwrap()),
        )
        .await;
    assert!(hand.is_delivered());
    let before = rig.effect_count();
    let run = rig.run().await;
    assert_eq!((run.state, run.stop_reason), (PublishState::Done, None));
    assert_eq!(
        labels(&rig.effects().split_off(before)),
        [format!("delete:{CHANNEL}:{}", id(1))],
        "only the message still there is deleted"
    );
    assert_eq!(
        outcomes(&rig.ops(&run).await),
        [
            (OpKind::Delete, 1, OpOutcome::Done),
            (OpKind::Delete, 0, OpOutcome::Gone),
        ]
    );
    assert!(rig.slots().await.is_empty());
}

/// A 429 on an edit or delete is retried only after the transport's
/// retry-after (bounded by the 30 s call deadline); a longer or unknown
/// wait stops the run with nothing changed.
#[tokio::test(start_paused = true)]
async fn a_rate_limited_edit_waits_out_its_retry_after_or_stops() {
    let rig = Rig::new().await;
    rig.save(CHANNEL, &[text(1, "A"), text(2, "B")]).await;
    rig.publish().await;

    rig.save(CHANNEL, &[text(1, "A2"), text(2, "B")]).await;
    rig.fake
        .script(Op::Edit, Step::Reject(RejectionKind::RateLimited));
    rig.fake.set_retry_after(Some(Duration::from_secs(5)));
    let started = Instant::now();
    let run = rig.publish().await;
    assert_eq!(run.state, PublishState::Done, "{run:?}");
    assert!(started.elapsed() >= Duration::from_secs(5), "waited it out");
    assert_eq!(rig.fake.count(Op::Edit), 2);

    rig.save(CHANNEL, &[text(1, "A3"), text(2, "B")]).await;
    let before = rig.slots().await;
    rig.fake
        .script(Op::Edit, Step::Reject(RejectionKind::RateLimited));
    rig.fake.set_retry_after(Some(Duration::from_secs(60)));
    let started = Instant::now();
    let run = rig.publish().await;
    assert_eq!(
        (run.state, run.stop_reason),
        (PublishState::Stopped, Some(StopReason::DiscordUnavailable))
    );
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "no wait past the deadline"
    );
    assert_eq!(rig.fake.count(Op::Edit), 3, "no early retry");
    assert_eq!(
        outcomes(&rig.ops(&run).await),
        [(OpKind::Edit, 0, OpOutcome::Skipped)]
    );
    assert_eq!(rig.slots().await, before);

    // A delete without a known retry-after (Twilight already waited what
    // it was told) is not retried either.
    rig.save(CHANNEL, &[text(1, "A3")]).await;
    rig.fake.set_retry_after(None);
    rig.fake
        .script(Op::Delete, Step::Reject(RejectionKind::RateLimited));
    let deletes = rig.fake.count(Op::Delete);
    let run = rig.publish().await;
    assert_eq!(run.stop_reason, Some(StopReason::DiscordUnavailable));
    assert_eq!(rig.fake.count(Op::Delete), deletes + 1);
    assert_eq!(rig.slots().await.len(), 2, "the slot stays");
}
