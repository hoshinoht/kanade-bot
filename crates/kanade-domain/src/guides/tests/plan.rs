use std::collections::BTreeSet;

use super::super::{
    Blocked, Confirmations, Grant, GuideEmbed, GuideGrants, Inspection, Plan, PlanCounts, PlanOp,
    PublishedSlot, RenderedSlot, SlotState, plan, plan_unpublish,
};
use super::support::{CHANNEL, OLD, desired, message, posted, published, with_state};

const ALL: GuideGrants = GuideGrants {
    view: true,
    send: true,
    history: true,
    embed: true,
    attach: true,
};

fn slots(hashes: &[&str]) -> Vec<RenderedSlot> {
    hashes.iter().map(|hash| desired(hash, &[])).collect()
}

fn inspect(gone: &[u32], newest: Option<&str>) -> Inspection {
    Inspection::Channel {
        grants: ALL,
        gone: gone.iter().copied().collect::<BTreeSet<u32>>(),
        newest_message_id: newest.map(str::to_owned),
    }
}

fn same_channel(p: &[PublishedSlot], d: &[RenderedSlot], inspection: Option<&Inspection>) -> Plan {
    plan(p, d, CHANNEL, true, inspection)
}

fn edit(position: u32, hash: &str) -> PlanOp {
    PlanOp::Edit {
        position,
        channel_id: CHANNEL.into(),
        message_id: message(position),
        hash: hash.into(),
    }
}

fn create(position: u32, hash: &str) -> PlanOp {
    PlanOp::Create {
        position,
        channel_id: CHANNEL.into(),
        hash: hash.into(),
    }
}

fn delete(position: u32, channel: &str) -> PlanOp {
    PlanOp::Delete {
        position,
        channel_id: channel.into(),
        message_id: message(position),
    }
}

fn forget(position: u32, channel: &str) -> PlanOp {
    PlanOp::Forget {
        position,
        channel_id: channel.into(),
    }
}

fn counts(keep: u32, edit: u32, create: u32, delete: u32) -> PlanCounts {
    PlanCounts {
        keep,
        edit,
        create,
        delete,
    }
}

#[test]
fn first_publish_creates_every_slot_in_order() {
    let out = same_channel(&[], &slots(&["a", "b", "c"]), None);
    assert_eq!(out.ops, [create(0, "a"), create(1, "b"), create(2, "c")]);
    assert!(out.needs.is_empty());
    assert!(!out.checked && !out.no_op());
    assert_eq!(out.counts, counts(0, 0, 3, 0));
}

#[test]
fn an_unchanged_guide_is_a_no_op() {
    let d = slots(&["a", "b", "c"]);
    let out = same_channel(&published(&d), &d, Some(&inspect(&[], Some(&message(2)))));
    assert!(out.no_op() && out.checked);
    assert_eq!(out.counts, counts(3, 0, 0, 0));
}

#[test]
fn only_changed_slots_are_edited() {
    let p = published(&slots(&["a", "b", "c"]));
    let out = same_channel(&p, &slots(&["a", "B", "c"]), None);
    assert_eq!(out.ops, [edit(1, "B")]);
    assert!(out.needs.is_empty());
    assert_eq!(out.counts, counts(2, 1, 0, 0));
}

#[test]
fn new_tail_slots_are_appended() {
    let p = published(&slots(&["a", "b"]));
    let out = same_channel(
        &p,
        &slots(&["a", "b", "c", "d"]),
        Some(&inspect(&[], Some(&message(1)))),
    );
    assert_eq!(out.ops, [create(2, "c"), create(3, "d")]);
    assert!(out.needs.is_empty());
}

#[test]
fn removed_tail_slots_are_deleted_newest_first() {
    let p = published(&slots(&["a", "b", "c", "d"]));
    let out = same_channel(&p, &slots(&["a", "b"]), None);
    assert_eq!(out.ops, [delete(3, CHANNEL), delete(2, CHANNEL)]);
    assert!(out.needs.is_empty());
    assert_eq!(out.counts, counts(2, 0, 0, 2));
}

#[test]
fn a_middle_insert_shifts_by_edits_and_appends_one() {
    let p = published(&slots(&["a", "b", "c"]));
    let out = same_channel(&p, &slots(&["a", "x", "b", "c"]), None);
    assert_eq!(out.ops, [edit(1, "x"), edit(2, "b"), create(3, "c")]);
    assert!(out.needs.is_empty());
}

#[test]
fn a_middle_removal_shifts_by_edits_and_deletes_the_tail() {
    let p = published(&slots(&["a", "b", "c"]));
    let out = same_channel(&p, &slots(&["a", "c"]), None);
    assert_eq!(out.ops, [edit(1, "c"), delete(2, CHANNEL)]);
    assert!(out.needs.is_empty());
}

#[test]
fn an_upload_change_in_the_middle_is_a_confirmed_repost() {
    let before = vec![
        desired("a", &[]),
        desired("b", &["x.png"]),
        desired("c", &[]),
    ];
    let after = vec![
        desired("a", &[]),
        desired("B", &["y.png"]),
        desired("c", &[]),
    ];
    let out = same_channel(&published(&before), &after, None);
    assert_eq!(
        out.ops,
        [
            delete(2, CHANNEL),
            delete(1, CHANNEL),
            create(1, "B"),
            create(2, "c")
        ]
    );
    assert_eq!(out.needs.repost_from, Some(1));
    assert_eq!(out.counts, counts(1, 0, 2, 2));
    // A middle delete runs only with exactly this confirmation.
    assert!(!Confirmations::default().covers(&out.needs));
    let wrong = Confirmations {
        repost_from: Some(0),
        ..Confirmations::default()
    };
    assert!(!wrong.covers(&out.needs));
    assert!(out.needs.covers(&out.needs));
}

#[test]
fn shifting_across_different_files_becomes_a_confirmed_repost() {
    let before = vec![
        desired("a", &[]),
        desired("b", &["x.png"]),
        desired("c", &[]),
    ];
    let after = vec![
        desired("a", &[]),
        desired("n", &[]),
        desired("b", &["x.png"]),
        desired("c", &[]),
    ];
    let out = same_channel(&published(&before), &after, None);
    assert_eq!(out.needs.repost_from, Some(1));
    assert_eq!(
        out.ops,
        [
            delete(2, CHANNEL),
            delete(1, CHANNEL),
            create(1, "n"),
            create(2, "b"),
            create(3, "c")
        ]
    );
}

#[test]
fn reposting_only_the_last_slot_needs_no_confirmation() {
    let before = vec![desired("a", &[]), desired("b", &["x.png"])];
    let after = vec![desired("a", &[]), desired("B", &["y.png"])];
    let out = same_channel(&published(&before), &after, None);
    assert_eq!(out.ops, [delete(1, CHANNEL), create(1, "B")]);
    assert!(out.needs.is_empty());
}

#[test]
fn a_gone_last_slot_is_posted_again_without_confirmation() {
    let d = slots(&["a", "b", "c"]);
    let out = same_channel(&published(&d), &d, Some(&inspect(&[2], Some(&message(1)))));
    assert_eq!(out.ops, [forget(2, CHANNEL), create(2, "c")]);
    assert!(out.needs.is_empty());
    // A stored gone state counts the same as an inspected one.
    let mut p = published(&d);
    p[2] = with_state(p[2].clone(), SlotState::Gone);
    assert_eq!(same_channel(&p, &d, None).ops, out.ops);
}

#[test]
fn a_gone_middle_slot_is_a_confirmed_repost() {
    let d = slots(&["a", "b", "c", "d"]);
    let out = same_channel(&published(&d), &d, Some(&inspect(&[1], Some(&message(3)))));
    assert_eq!(
        out.ops,
        [
            delete(3, CHANNEL),
            delete(2, CHANNEL),
            forget(1, CHANNEL),
            create(1, "b"),
            create(2, "c"),
            create(3, "d")
        ]
    );
    assert_eq!(out.needs.repost_from, Some(1));
    // Decision 10: a gone slot followed only by the posted last slot is a
    // middle slot too.
    let short = slots(&["a", "b", "c"]);
    let out = same_channel(&published(&short), &short, Some(&inspect(&[1], None)));
    assert_eq!(out.needs.repost_from, Some(1));
    // Gone from the middle to the end is a gone tail: nothing posted goes.
    let out = same_channel(&published(&short), &short, Some(&inspect(&[1, 2], None)));
    assert_eq!(
        out.ops,
        [
            forget(2, CHANNEL),
            forget(1, CHANNEL),
            create(1, "b"),
            create(2, "c")
        ]
    );
    assert!(out.needs.is_empty());
}

#[test]
fn appending_after_foreign_messages_needs_confirmation() {
    let p = published(&slots(&["a", "b"]));
    let d = slots(&["a", "b", "c"]);
    let foreign = "99999999999999999999";
    let out = same_channel(&p, &d, Some(&inspect(&[], Some(foreign))));
    assert!(out.needs.interleaved);
    assert_eq!(out.ops, [create(2, "c")]);
    // Our own last message is the newest: no confirmation.
    let out = same_channel(&p, &d, Some(&inspect(&[], Some(&message(1)))));
    assert!(!out.needs.interleaved);
    // Unchecked plans cannot know; the worker re-plans with an inspection.
    assert!(!same_channel(&p, &d, None).needs.interleaved);
    // An edit-only plan appends nothing; a first publish follows nothing.
    let edit_only = same_channel(&p, &slots(&["a", "B"]), Some(&inspect(&[], Some(foreign))));
    assert!(!edit_only.needs.interleaved);
    let first = same_channel(&[], &d, Some(&inspect(&[], Some(foreign))));
    assert!(!first.needs.interleaved);
}

#[test]
fn a_channel_move_posts_the_new_set_then_deletes_the_old() {
    let d = slots(&["a", "b"]);
    let mut p: Vec<PublishedSlot> = slots(&["a", "b", "c"])
        .iter()
        .zip(0..)
        .map(|(slot, position)| posted(position, OLD, slot))
        .collect();
    p[1] = with_state(p[1].clone(), SlotState::Gone);
    let out = plan(&p, &d, CHANNEL, true, None);
    assert_eq!(
        out.ops,
        [
            create(0, "a"),
            create(1, "b"),
            delete(2, OLD),
            forget(1, OLD),
            delete(0, OLD)
        ]
    );
    assert_eq!(
        out.needs,
        Confirmations {
            channel_move: true,
            ..Confirmations::default()
        }
    );
    assert_eq!(out.counts, counts(0, 0, 2, 2));
    // Without delete_old the old messages stay and only their rows go.
    let kept = plan(&p, &d, CHANNEL, false, None);
    assert_eq!(
        kept.ops,
        [
            create(0, "a"),
            create(1, "b"),
            forget(2, OLD),
            forget(1, OLD),
            forget(0, OLD)
        ]
    );
    assert!(kept.needs.channel_move);
    assert_ne!(kept.hash, out.hash);
}

#[test]
fn an_interrupted_channel_move_continues_where_it_stopped() {
    let d = slots(&["a", "b", "c"]);
    let mut p = vec![posted(0, CHANNEL, &d[0])];
    p.extend((0..3).map(|position| posted(position, OLD, &d[position as usize])));
    let out = plan(&p, &d, CHANNEL, true, None);
    assert_eq!(
        out.ops,
        [
            create(1, "b"),
            create(2, "c"),
            delete(2, OLD),
            delete(1, OLD),
            delete(0, OLD)
        ]
    );
    assert!(out.needs.channel_move);
}

#[test]
fn an_unknown_slot_or_missing_channel_blocks_with_no_ops() {
    let d = slots(&["a", "b"]);
    let mut p = published(&d);
    p[1] = with_state(p[1].clone(), SlotState::Unknown);
    let out = same_channel(&p, &d, None);
    assert_eq!(
        out.blocked,
        Some(Blocked::UnknownSlot {
            channel_id: CHANNEL.into(),
            position: 1
        })
    );
    assert!(out.ops.is_empty() && !out.no_op());
    let out = same_channel(
        &published(&d),
        &slots(&["x"]),
        Some(&Inspection::ChannelMissing),
    );
    assert_eq!(out.blocked, Some(Blocked::ChannelMissing));
    assert!(out.ops.is_empty());
}

#[test]
fn missing_grants_block_before_any_call() {
    let embed = GuideEmbed {
        title: "t".into(),
        description: "d".into(),
        colour: 0,
        thumbnail: Some("x.png".into()),
        image: None,
    };
    let mut d = vec![desired("a", &["x.png"])];
    d[0].embeds.push(embed);
    let grants = GuideGrants {
        attach: false,
        history: false,
        ..ALL
    };
    let inspection = Inspection::Channel {
        grants,
        gone: BTreeSet::new(),
        newest_message_id: None,
    };
    let out = same_channel(&[], &d, Some(&inspection));
    assert_eq!(
        out.blocked,
        Some(Blocked::Grants(vec![Grant::History, Grant::Attach]))
    );
    assert_eq!(out.ops, [create(0, "a")]);
    // A no-op needs nothing.
    let p = published(&d);
    assert!(same_channel(&p, &d, Some(&inspection)).no_op());
}

#[test]
fn plan_hashes_are_stable_and_cover_ops_and_needs() {
    let p = published(&slots(&["a", "b"]));
    let d = slots(&["a", "B", "c"]);
    let first = same_channel(&p, &d, None);
    assert_eq!(first.hash, same_channel(&p, &d, None).hash);
    // Pinned: a changed canonical form would stop every queued run.
    assert_eq!(
        first.hash,
        "1f3ef232053248e63c62b13f0e8287805984a3287b419f5740a61ce74c5f48b9"
    );
    let foreign = same_channel(&p, &d, Some(&inspect(&[], Some("99999999999999999999"))));
    assert_eq!(foreign.ops, first.ops);
    assert_ne!(foreign.hash, first.hash);
    assert_ne!(
        same_channel(&p, &slots(&["a", "C", "c"]), None).hash,
        first.hash
    );
    assert_ne!(
        plan(&p, &d, "100000000000000009", true, None).hash,
        first.hash
    );
}

#[test]
fn unpublish_deletes_everything_newest_first() {
    let d = slots(&["a", "b", "c"]);
    let mut p = published(&d);
    p[1] = with_state(p[1].clone(), SlotState::Gone);
    let out = plan_unpublish(&p, CHANNEL);
    assert_eq!(
        out.ops,
        [delete(2, CHANNEL), forget(1, CHANNEL), delete(0, CHANNEL)]
    );
    assert!(out.needs.is_empty());
    p[0] = with_state(p[0].clone(), SlotState::Unknown);
    assert!(matches!(
        plan_unpublish(&p, CHANNEL).blocked,
        Some(Blocked::UnknownSlot { position: 0, .. })
    ));
}
