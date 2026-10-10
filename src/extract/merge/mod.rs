//! Folding one burst's per-message amendments into one candidate per affected
//! run (v4 `bot/extract/merge.py`).
//!
//! Only amendments of the same kind about the same target merge; an `rsvp` is
//! keyed by who answered. The latest message wins each field, so "amend to
//! 9:45pm" beats the "9pm" before it.

use std::collections::HashMap;

use crate::extract::{Amendment, AmendmentKind};

/// `(evidence order, input index, amendment)`: the merge's recency order.
type Member<'a> = (i64, usize, &'a Amendment);

#[derive(PartialEq, Eq)]
enum Key {
    /// One answer per person set.
    Rsvp(Vec<String>),
    Other(AmendmentKind, Vec<String>, String),
}

fn key(amendment: &Amendment) -> Key {
    let sorted = |items: &[String]| {
        let mut items = items.to_vec();
        items.sort();
        items
    };
    if amendment.kind == AmendmentKind::Rsvp {
        return Key::Rsvp(sorted(&amendment.participants));
    }
    Key::Other(
        amendment.kind,
        sorted(&amendment.bosses),
        amendment.target_run_hint.clone().unwrap_or_default(),
    )
}

/// How late in the burst an amendment's evidence is; -1 when none is known.
fn order(amendment: &Amendment, positions: &HashMap<&str, i64>) -> i64 {
    amendment
        .evidence_message_ids
        .iter()
        .map(|id| positions.get(id.as_str()).copied().unwrap_or(-1))
        .max()
        .unwrap_or(-1)
}

fn push_new(into: &mut Vec<String>, values: &[String]) {
    for value in values {
        if !into.contains(value) {
            into.push(value.clone());
        }
    }
}

fn is_set(value: Option<&String>) -> bool {
    value.is_some_and(|text| !text.is_empty())
}

/// Fold a burst's amendments into one per affected run, latest value winning.
///
/// `message_order` is the burst's message ids oldest first (without it input
/// order decides "latest"). `existing_bosses` are the boss lists of the runs
/// already scheduled in the channel: a `move` matching none of them is part of
/// the `add` proposing that run.
pub fn merge<S: AsRef<str>>(
    amendments: &[Amendment],
    message_order: &[S],
    existing_bosses: &[Vec<String>],
) -> Vec<Amendment> {
    let positions: HashMap<&str, i64> = (0..)
        .zip(message_order)
        .map(|(index, id)| (id.as_ref(), index))
        .collect();

    let mut groups: Vec<(Key, Vec<Member<'_>>)> = Vec::new();
    for (index, amendment) in amendments.iter().enumerate() {
        let entry = (order(amendment, &positions), index, amendment);
        let key = key(amendment);
        match groups.iter_mut().find(|(known, _)| *known == key) {
            Some((_, members)) => members.push(entry),
            None => groups.push((key, vec![entry])),
        }
    }

    let mut out: Vec<Amendment> = groups
        .into_iter()
        .map(|(_, mut members)| {
            members.sort_by_key(|&(order, index, _)| (order, index));
            merge_group(&members)
        })
        .collect();

    out = fold_time_changes_into_adds(out, &positions, existing_bosses);
    carry_time_across_moves(&mut out);
    out.sort_by_key(|amendment| order(amendment, &positions));
    out
}

fn merge_group(members: &[Member<'_>]) -> Amendment {
    let newest = members[members.len() - 1].2;
    let latest = |field: fn(&Amendment) -> Option<&String>| {
        members.iter().rev().find_map(|(_, _, a)| field(a)).cloned()
    };
    let union = |field: fn(&Amendment) -> &Vec<String>| {
        let mut seen = Vec::new();
        for (_, _, amendment) in members {
            push_new(&mut seen, field(amendment));
        }
        seen
    };
    Amendment {
        kind: newest.kind,
        bosses: union(|a| &a.bosses),
        day_ref: latest(|a| a.day_ref.as_ref()),
        time_ref: latest(|a| a.time_ref.as_ref()),
        participants: union(|a| &a.participants),
        rsvp: members.iter().rev().find_map(|(_, _, a)| a.rsvp),
        // A burst ending on an answer is no longer an open question.
        is_question: newest.is_question,
        confidence: members
            .iter()
            .map(|(_, _, a)| a.confidence)
            .fold(f64::NEG_INFINITY, f64::max),
        evidence_message_ids: union(|a| &a.evidence_message_ids),
        target_run_hint: latest(|a| a.target_run_hint.as_ref()),
    }
}

fn matches_an_existing_run(amendment: &Amendment, existing_bosses: &[Vec<String>]) -> bool {
    if amendment.bosses.is_empty() {
        // No bosses named: it could be about anything already scheduled.
        return !existing_bosses.is_empty();
    }
    existing_bosses
        .iter()
        .any(|bosses| shares_a_boss(&amendment.bosses, bosses))
}

fn shares_a_boss(a: &[String], b: &[String]) -> bool {
    a.iter().any(|boss| b.contains(boss))
}

/// A later `move` about a run that does not exist yet settles the `add` it
/// shares a boss with: one card at the final time, not a new run plus a move
/// nothing can apply to.
fn fold_time_changes_into_adds(
    mut candidates: Vec<Amendment>,
    positions: &HashMap<&str, i64>,
    existing_bosses: &[Vec<String>],
) -> Vec<Amendment> {
    let adds: Vec<usize> = (0..candidates.len())
        .filter(|&index| candidates[index].kind == AmendmentKind::Add)
        .collect();
    if adds.is_empty() {
        return candidates;
    }
    let mut keep: Vec<usize> = Vec::new();
    for index in 0..candidates.len() {
        let amendment = &candidates[index];
        if amendment.kind != AmendmentKind::Move
            || matches_an_existing_run(amendment, existing_bosses)
        {
            keep.push(index);
            continue;
        }
        // Orders are read live: an earlier fold may have extended an add's evidence.
        let target = adds.iter().copied().find(|&add| {
            shares_a_boss(&candidates[add].bosses, &amendment.bosses)
                && order(amendment, positions) >= order(&candidates[add], positions)
        });
        let Some(target) = target else {
            keep.push(index);
            continue;
        };
        let moved = candidates[index].clone();
        let add = &mut candidates[target];
        if moved.day_ref.is_some() {
            add.day_ref = moved.day_ref;
        }
        if moved.time_ref.is_some() {
            add.time_ref = moved.time_ref;
        }
        push_new(&mut add.evidence_message_ids, &moved.evidence_message_ids);
        push_new(&mut add.participants, &moved.participants);
        add.confidence = add.confidence.max(moved.confidence);
        add.is_question = moved.is_question;
    }
    keep.into_iter()
        .map(|index| candidates[index].clone())
        .collect()
}

/// One time stated for a day applies to every run moved to that day, but only
/// when the day is written the same way and exactly one time was given for it.
fn carry_time_across_moves(candidates: &mut [Amendment]) {
    let mut by_day: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, amendment) in candidates.iter().enumerate() {
        if amendment.kind != AmendmentKind::Move {
            continue;
        }
        let day = crate::domain::pytext::strip(amendment.day_ref.as_deref().unwrap_or_default())
            .to_lowercase();
        if day.is_empty() {
            continue;
        }
        match by_day.iter_mut().find(|(known, _)| *known == day) {
            Some((_, group)) => group.push(index),
            None => by_day.push((day, vec![index])),
        }
    }
    for (_, group) in by_day {
        let (stated, missing): (Vec<usize>, Vec<usize>) = group
            .into_iter()
            .partition(|&index| is_set(candidates[index].time_ref.as_ref()));
        let [source] = stated[..] else {
            continue;
        };
        let source = candidates[source].clone();
        for index in missing {
            let amendment = &mut candidates[index];
            amendment.time_ref.clone_from(&source.time_ref);
            amendment.is_question = amendment.is_question && source.is_question;
            push_new(
                &mut amendment.evidence_message_ids,
                &source.evidence_message_ids,
            );
        }
    }
}
