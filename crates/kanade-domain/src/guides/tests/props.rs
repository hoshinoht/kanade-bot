//! Plan properties over every small slot map (exhaustive, no generator): up
//! to three published slots (posted or gone, two contents, with or without
//! a file) against up to three desired slots, with and without a foreign
//! newest message, and channel moves (interrupted ones included). Gone
//! slots reported by an inspection take the stored-gone path (pinned in the
//! plan cases), so stored states cover them here.

use std::collections::BTreeMap;

use super::super::{
    GuideGrants, Inspection, OpKind, Plan, PlanOp, PublishedSlot, RenderedSlot, SlotState,
    UploadRef, plan,
};
use super::support::{CHANNEL, OLD, at, desired, posted, with_state};

const FOREIGN: &str = "99999999999999999999";

/// Every desired slot: content `a`/`b`, with or without `f.png`.
fn outputs() -> Vec<RenderedSlot> {
    ["a", "b"]
        .iter()
        .flat_map(|content| {
            [
                desired(content, &[]),
                desired(&format!("{content}f"), &["f.png"]),
            ]
        })
        .collect()
}

/// Every sequence of `len` items from `items`.
fn sequences<T: Clone>(items: &[T], len: usize) -> Vec<Vec<T>> {
    (0..len).fold(vec![Vec::new()], |acc, _| {
        acc.iter()
            .flat_map(|prefix| {
                items.iter().map(move |item| {
                    let mut next = prefix.clone();
                    next.push(item.clone());
                    next
                })
            })
            .collect()
    })
}

/// A slot row as the model keeps it; `message_id` only while posted.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    hash: String,
    uploads: Vec<UploadRef>,
    message_id: Option<String>,
}

type Rows = BTreeMap<(String, u32), Row>;

/// Apply `plan` to `published` as the worker and store do when every
/// effect succeeds, checking each op is legal on the way.
fn apply(published: &[PublishedSlot], plan: &Plan, desired: &[RenderedSlot]) -> Rows {
    let mut rows: Rows = published
        .iter()
        .map(|slot| {
            let row = Row {
                hash: slot.hash.clone(),
                uploads: slot.uploads.clone(),
                message_id: slot
                    .message_id
                    .clone()
                    .filter(|_| slot.state == SlotState::Posted),
            };
            ((slot.channel_id.clone(), slot.position), row)
        })
        .collect();
    let mut next_message = 5000;
    for op in &plan.ops {
        let key = (op.channel_id().to_owned(), op.position());
        match op {
            PlanOp::Edit {
                message_id, hash, ..
            } => {
                let row = rows.get_mut(&key).expect("edit of a missing row");
                assert_eq!(
                    row.message_id.as_ref(),
                    Some(message_id),
                    "edit of a gone message"
                );
                let slot = &desired[op.position() as usize];
                assert_eq!(&slot.hash, hash);
                assert_eq!(row.uploads, slot.manifest(), "an edit changed the files");
                row.hash.clone_from(hash);
            }
            PlanOp::Create { hash, .. } => {
                next_message += 1;
                let row = Row {
                    hash: hash.clone(),
                    uploads: desired[op.position() as usize].manifest(),
                    message_id: Some(next_message.to_string()),
                };
                assert!(rows.insert(key, row).is_none(), "create over a row");
            }
            PlanOp::Delete { message_id, .. } => {
                let row = rows.remove(&key).expect("delete of a missing row");
                assert_eq!(row.message_id.as_ref(), Some(message_id));
            }
            PlanOp::Forget { channel_id, .. } => {
                let row = rows.remove(&key).expect("forget of a missing row");
                // Only an old channel's posted messages may be left behind.
                assert!(row.message_id.is_none() || channel_id != CHANNEL);
            }
        }
    }
    rows
}

/// The rows' hashes, which must be exactly `desired` in [`CHANNEL`].
fn assert_reaches(rows: &Rows, desired: &[RenderedSlot], label: &dyn Fn() -> String) {
    let got: Vec<(&str, u32, &str)> = rows
        .iter()
        .map(|((channel, position), row)| (channel.as_str(), *position, row.hash.as_str()))
        .collect();
    let want: Vec<(&str, u32, &str)> = desired
        .iter()
        .zip(0..)
        .map(|(slot, position)| (CHANNEL, position, slot.hash.as_str()))
        .collect();
    assert_eq!(got, want, "{}", label());
    assert!(
        rows.values().all(|row| row.message_id.is_some()),
        "{}",
        label()
    );
}

/// No create precedes a removal in the same channel.
fn assert_creates_last(out: &Plan, label: &dyn Fn() -> String) {
    for (at, op) in out.ops.iter().enumerate() {
        if op.kind() == OpKind::Create {
            let later_removal = out.ops[at..].iter().any(|later| {
                later.channel_id() == op.channel_id()
                    && matches!(later.kind(), OpKind::Delete | OpKind::Forget)
            });
            assert!(!later_removal, "{}", label());
        }
    }
}

/// The slot map after `rows` were applied.
fn republished(rows: &Rows) -> Vec<PublishedSlot> {
    rows.iter()
        .map(|((channel, position), row)| PublishedSlot {
            position: *position,
            channel_id: channel.clone(),
            message_id: row.message_id.clone(),
            hash: row.hash.clone(),
            uploads: row.uploads.clone(),
            state: SlotState::Posted,
            source: None,
            publish_id: "next".into(),
            updated_at: at(),
        })
        .collect()
}

fn inspection(newest: Option<&str>) -> Inspection {
    Inspection::Channel {
        grants: GuideGrants {
            view: true,
            send: true,
            history: true,
            embed: true,
            attach: true,
        },
        gone: Default::default(),
        newest_message_id: newest.map(str::to_owned),
    }
}

#[test]
fn every_small_same_channel_plan_reaches_the_desired_slots() {
    let outputs = outputs();
    let states: Vec<(RenderedSlot, SlotState)> = outputs
        .iter()
        .flat_map(|slot| [SlotState::Posted, SlotState::Gone].map(|state| (slot.clone(), state)))
        .collect();
    let mut checked = 0;
    for m in 0..=3 {
        for before in sequences(&states, m) {
            let published: Vec<PublishedSlot> = before
                .iter()
                .zip(0..)
                .map(|((slot, state), position)| {
                    with_state(posted(position, CHANNEL, slot), *state)
                })
                .collect();
            for k in 0..=3 {
                for after in sequences(&outputs, k) {
                    for newest in [None, Some(FOREIGN)] {
                        let out =
                            plan(&published, &after, CHANNEL, true, Some(&inspection(newest)));
                        let label = || format!("{published:?}\n{after:?}\n{:?}", out.ops);
                        check(&published, &after, &out, &label);
                        // Interleaving is asked only for a create after a
                        // foreign newest message.
                        if out.needs.interleaved {
                            assert_eq!(newest, Some(FOREIGN), "{}", label());
                            assert!(out.counts.create > 0, "{}", label());
                        }
                        // Once applied, the same guide plans nothing.
                        let rows = apply(&published, &out, &after);
                        let again = plan(&republished(&rows), &after, CHANNEL, true, None);
                        assert!(again.no_op(), "{}\n{:?}", label(), again.ops);
                        checked += 1;
                    }
                }
            }
        }
    }
    assert!(checked > 50_000, "{checked}");
}

/// The same-channel properties of one plan.
fn check(
    published: &[PublishedSlot],
    desired: &[RenderedSlot],
    out: &Plan,
    label: &dyn Fn() -> String,
) {
    assert_eq!(out.blocked, None, "{}", label());
    assert_reaches(&apply(published, out, desired), desired, label);
    assert_creates_last(out, label);
    // Edits come before removals, which come before creates.
    let rank = |op: &PlanOp| match op.kind() {
        OpKind::Edit => 0,
        OpKind::Delete | OpKind::Forget => 1,
        OpKind::Create => 2,
    };
    assert!(
        out.ops
            .windows(2)
            .all(|pair| rank(&pair[0]) <= rank(&pair[1])),
        "{}",
        label()
    );

    // repost_from is set iff a posted message is deleted and the guide is
    // posted again from a slot before the last (decisions 7, 10): tail
    // deletes and last-slot reposts need none.
    let m = u32::try_from(published.len()).unwrap();
    let first_create = out
        .ops
        .iter()
        .filter(|op| op.kind() == OpKind::Create)
        .map(PlanOp::position)
        .min();
    let deletes = |middle_only: bool| {
        out.ops
            .iter()
            .any(|op| op.kind() == OpKind::Delete && (!middle_only || op.position() + 1 < m))
    };
    let reposted = first_create.filter(|&r| r < m && deletes(false));
    assert_eq!(
        out.needs.repost_from,
        reposted.filter(|&r| r + 1 < m),
        "{}",
        label()
    );
    // In particular: deleting a posted non-last message to post again
    // always asks.
    if deletes(true) && first_create.is_some() {
        assert!(out.needs.repost_from.is_some(), "{}", label());
    }

    // A no-op is exactly a slot map that already shows the guide.
    let matches = published.len() == desired.len()
        && published
            .iter()
            .zip(desired)
            .all(|(slot, want)| slot.state == SlotState::Posted && slot.hash == want.hash);
    assert_eq!(out.no_op(), matches, "{}", label());
}

#[test]
fn every_small_channel_move_reaches_the_desired_slots() {
    let outputs = outputs();
    for m in 1..=3 {
        for before in sequences(&[SlotState::Posted, SlotState::Gone], m) {
            let old: Vec<PublishedSlot> = before
                .iter()
                .zip(0..)
                .map(|(state, position)| with_state(posted(position, OLD, &outputs[0]), *state))
                .collect();
            let posted_old = before
                .iter()
                .filter(|state| **state == SlotState::Posted)
                .count();
            for k in 0..=3 {
                for after in sequences(&outputs, k) {
                    // `done` slots already created in the new channel by an
                    // interrupted move, the old set still in place.
                    for done in 0..=k {
                        let mut published = old.clone();
                        published.extend(
                            after
                                .iter()
                                .take(done)
                                .zip(0..)
                                .map(|(slot, position)| posted(position, CHANNEL, slot)),
                        );
                        for delete_old in [true, false] {
                            let out = plan(&published, &after, CHANNEL, delete_old, None);
                            let label = || format!("{published:?}\n{after:?}\n{:?}", out.ops);
                            assert!(out.needs.channel_move, "{}", label());
                            assert_eq!(out.needs.repost_from, None, "{}", label());
                            assert_reaches(&apply(&published, &out, &after), &after, &label);
                            assert_creates_last(&out, &label);
                            // Old messages are deleted only with delete_old,
                            // and only after every create.
                            let deletes = out
                                .ops
                                .iter()
                                .filter(|op| op.channel_id() == OLD && op.kind() == OpKind::Delete)
                                .count();
                            assert_eq!(deletes, if delete_old { posted_old } else { 0 });
                            let last_create =
                                out.ops.iter().rposition(|op| op.kind() == OpKind::Create);
                            let first_old = out.ops.iter().position(|op| op.channel_id() == OLD);
                            if let (Some(create), Some(old)) = (last_create, first_old) {
                                assert!(create < old, "{}", label());
                            }
                        }
                    }
                }
            }
        }
    }
}
