//! Draft operations: the stored codec, replay, rewinding history to a base
//! and the three-way merge into the current schedule.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, TimeZone, Utc, Weekday};
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::{
    drafts::{
        DRAFT_OP_FORMAT, DraftOp, Entity, Field, FieldValue, MergeAnalysis, MergeConflict, Removal,
        ReplayError, Target, analyze_merge, analyze_merge_since, decode, encode, replay,
    },
    history::{
        ChangeFilter, ChangeHistory, ChangeQuery, ChangeRecord, ChangeRef, HistoryGap, MAX_PAGE,
        rewind,
    },
    schedule::{
        AmendedRunChoice, FixedEdit, FixedEditChoices, FixedEditRequest, FixedField, NewFixedRun,
        RsvpState, RunSource, RunStatus, ScheduleError, ScheduleSnapshot, StatusChange,
    },
    scheduler::SchedulerService,
};
use serde_json::{Value, json};

use crate::common::TestClock;
use crate::support::{
    CountingIds, Guild, amend, by_tests, existing, hm, kalos_run, kl, now, one_timing as fixture,
    owned, policy, rsvp, snapshot, swap, utc, weekly,
};

type Service = SchedulerService<SqliteStore, CountingIds, TestClock>;

fn id(target: &Target) -> &str {
    match target {
        Target::Existing(id) => id,
        Target::Created(_) => panic!("upstream operations name existing rows"),
    }
}

/// Commit one operation through the service, as another member would.
async fn commit(service: &mut Service, op: &DraftOp) {
    let policy = policy();
    let origin = by_tests(service);
    let committed = match op {
        DraftOp::AddFixedRun(new) => origin.add_fixed_run(new.clone()).await.map(drop),
        DraftOp::ApplyFixedEdit {
            fixed,
            edit,
            choices,
        } => {
            let request = FixedEditRequest {
                fixed_id: id(fixed).to_owned(),
                edit: edit.clone(),
                choices: choices.clone(),
            };
            let applied = origin.apply_fixed_edit(&request, &Guild, &policy);
            applied.await.map(drop)
        }
        // The real delta operation, as an approved request commits it.
        DraftOp::FixedParticipants { fixed, add, remove } => origin
            .change_fixed_party(id(fixed), add, remove, &Guild, &policy)
            .await
            .map(drop),
        DraftOp::RetireFixedRun { fixed, weeks } => origin
            .retire_fixed_run(id(fixed), weeks, &policy.reminders)
            .await
            .map(drop),
        DraftOp::CreateRun { .. } => panic!("not used upstream"),
        DraftOp::AmendRun { run, to } => origin.amend_run(id(run), *to, &policy).await.map(drop),
        DraftOp::SetStatus { run, change } => origin
            .set_status(id(run), *change, &policy.reminders)
            .await
            .map(drop),
        DraftOp::SwapParticipants {
            run,
            remove,
            add,
            via_portal,
        } => origin
            .swap_participants(id(run), remove, add, *via_portal, &Guild)
            .await
            .map(drop),
        DraftOp::SetRsvp {
            run,
            user_id,
            state,
            source,
        } => origin.set_rsvp(id(run), user_id, *state, *source).await,
        DraftOp::ResetToFixed { run } => origin.reset_to_fixed(id(run), &policy).await.map(drop),
        DraftOp::SetRunBosses { .. }
        | DraftOp::EnsureReminders { .. }
        | DraftOp::RecountRun { .. }
        | DraftOp::ReviveRun { .. } => panic!("proposal-only; not used upstream"),
    };
    committed.unwrap();
}

/// Analyse `draft` drafted on the fixture after `upstream` was committed.
async fn merge(upstream: &[DraftOp], draft: &[DraftOp]) -> (MergeAnalysis, ScheduleSnapshot) {
    let mut f = fixture().await;
    let base = snapshot(&f.service).await;
    for op in upstream {
        commit(&mut f.service, op).await;
    }
    let current = snapshot(&f.service).await;
    let analysis = analyze_merge(&base, &current, draft, &policy(), None, &Guild, now());
    (analysis, current)
}

fn status(run: &str, status: RunStatus) -> DraftOp {
    DraftOp::SetStatus {
        run: existing(run),
        change: StatusChange {
            status,
            announce: false,
            via_portal: true,
        },
    }
}

fn fixed_edit(fixed: &str, edit: FixedEdit) -> DraftOp {
    DraftOp::ApplyFixedEdit {
        fixed: existing(fixed),
        edit,
        choices: FixedEditChoices::UpdateAll,
    }
}

fn note(text: &str) -> FixedEdit {
    FixedEdit {
        note: Some(text.to_owned()),
        ..FixedEdit::default()
    }
}

fn bosses(names: &[&str]) -> FixedEdit {
    FixedEdit {
        bosses: Some(owned(names)),
        ..FixedEdit::default()
    }
}

fn members(ids: &[&str]) -> FieldValue {
    FieldValue::Set(owned(ids))
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    owned(ids).into_iter().collect()
}

fn participants(state: &ScheduleSnapshot, run: &str) -> BTreeSet<String> {
    let row = state.runs.iter().find(|row| row.id == run).unwrap();
    row.participants.iter().cloned().collect()
}

// ---- codec ----------------------------------------------------------------

fn codec_samples() -> Vec<DraftOp> {
    let at = utc(kl(31, 21, 30));
    vec![
        DraftOp::AddFixedRun(NewFixedRun {
            bosses: vec!["HFA".into(), "Kalos".into()],
            ..weekly("222", Weekday::Sat, hm(20, 15), &["1001", "1002"])
        }),
        DraftOp::ApplyFixedEdit {
            fixed: Target::Created(0),
            edit: FixedEdit {
                weekday: Some(Weekday::Sun),
                time: Some(hm(21, 0)),
                note: Some("prog \"night\"".into()),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::PerRun(BTreeMap::from([
                ("run-a".to_owned(), AmendedRunChoice::KeepForThisWeek),
                ("run-b".to_owned(), AmendedRunChoice::UpdateToFixed),
            ])),
        },
        DraftOp::ApplyFixedEdit {
            fixed: existing("fixed-1"),
            edit: FixedEdit {
                participants: Some(vec!["1003".into()]),
                channel_id: Some("333".into()),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::UpdateAll,
        },
        DraftOp::RetireFixedRun {
            fixed: existing("fixed-1"),
            weeks: vec![utc(kl(27, 0, 0)), utc(kl(3, 0, 0))],
        },
        DraftOp::CreateRun {
            fixed: Some(Target::Created(0)),
            channel_id: None,
            week_start: utc(kl(27, 0, 0)),
            datetime: at,
            bosses: vec!["HFA".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
        DraftOp::AmendRun {
            run: Target::Created(4),
            to: utc(kl(1, 20, 0)),
        },
        status("run-1", RunStatus::Cancelled),
        swap("run-1", &["1002"], &["1003", "1004"]),
        rsvp("run-1", "1003", RsvpState::Maybe),
        DraftOp::ResetToFixed {
            run: existing("run-1"),
        },
        DraftOp::FixedParticipants {
            fixed: existing("fixed-1"),
            add: vec!["1004".into()],
            remove: vec!["1002".into()],
        },
        DraftOp::SetRunBosses {
            run: existing("run-1"),
            bosses: vec!["HFA".into(), "Kalos \"C\"".into()],
        },
        DraftOp::EnsureReminders {
            run: Target::Created(4),
        },
        DraftOp::RecountRun {
            run: existing("run-1"),
        },
        DraftOp::ReviveRun {
            run: existing("run-1"),
        },
    ]
}

/// The pinned `kanade.draft_op.v1` strings, cross-checked with Python's
/// `json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=True)`.
/// Never regenerate it to make a failing test pass: a change to the encoder's
/// output is a new format version. Print the encoder's output with
/// `KANADE_PRINT_GOLDEN=1 cargo test --all-features --test scheduler codec_matches -- --nocapture`.
fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drafts/draft_ops.json")
}

#[test]
fn codec_matches_the_pinned_v1_encoding_and_round_trips() {
    let encoded: Vec<String> = codec_samples()
        .iter()
        .map(|op| encode(op).unwrap())
        .collect();
    if std::env::var_os("KANADE_PRINT_GOLDEN").is_some() {
        let file = json!({"format": DRAFT_OP_FORMAT, "ops": encoded});
        println!("{}", serde_json::to_string_pretty(&file).unwrap());
    }
    let golden: Value =
        serde_json::from_str(&std::fs::read_to_string(golden_path()).unwrap()).unwrap();
    assert_eq!(golden["format"], DRAFT_OP_FORMAT);
    let pinned: Vec<&str> = golden["ops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| op.as_str().unwrap())
        .collect();
    assert_eq!(encoded, pinned);
    for (op, text) in codec_samples().iter().zip(&pinned) {
        assert_eq!(&decode(text).unwrap(), op);
    }
}

#[test]
fn codec_refuses_other_formats_and_unknown_operations() {
    let text = encode(&status("run-1", RunStatus::Done)).unwrap();
    assert!(decode(&text.replace("draft_op.v1", "draft_op.v2")).is_err());
    assert!(decode(&text.replace("set_status", "drop_table")).is_err());
    assert!(decode("{").is_err());
}

#[test]
fn codec_is_strict_about_keys_times_targets_and_instants() {
    let pinned: Vec<String> = codec_samples()
        .iter()
        .map(|op| encode(op).unwrap())
        .collect();
    // (sample, key path, new value; `None` removes the key)
    let mut cases: Vec<(usize, &[&str], Option<Value>)> = vec![
        // Unknown keys, at the top and nested.
        (6, &["extra"], Some(json!(1))),
        (0, &["fixed_run", "extra"], Some(json!(1))),
        (1, &["edit", "owner_id"], Some(json!("1"))),
        (6, &["change", "extra"], Some(json!(true))),
        (1, &["choices", "all"], Some(json!(true))),
        // Missing optionals must be written as null.
        (0, &["fixed_run", "note"], None),
        // Targets hold exactly one of `existing` or `created`.
        (6, &["run", "created"], Some(json!(0))),
        (6, &["run"], Some(json!({}))),
        (6, &["run"], Some(json!({"existing": 7}))),
        (5, &["run"], Some(json!({"created": -1}))),
        // Instants only in the encoder's spelling.
        (5, &["to"], Some(json!("2026-09-01T20:00:00+08:00"))),
    ];
    // Only exact HH:MM:SS.
    let times = [
        "20:15",
        "20:15:00.0",
        "2:15:00",
        "20:15:0x",
        "24:00:00",
        " 20:15:0",
        "+1:15:00",
    ];
    cases.extend(times.map(|bad| (0, &["fixed_run", "time"][..], Some(json!(bad)))));
    for (sample, path, value) in cases {
        let mut op: Value = serde_json::from_str(&pinned[sample]).unwrap();
        let (key, parents) = path.split_last().unwrap();
        let parent = parents.iter().fold(&mut op, |node, key| &mut node[*key]);
        match &value {
            Some(value) => parent[*key] = value.clone(),
            None => drop(parent.as_object_mut().unwrap().remove(*key)),
        }
        assert!(
            decode(&op.to_string()).is_err(),
            "sample {sample}: {path:?} = {value:?}"
        );
    }
    // The untouched samples still decode.
    for text in &pinned {
        decode(text).unwrap();
    }
}

// ---- replay -----------------------------------------------------------------

#[tokio::test]
async fn replay_is_deterministic_resolves_created_targets_and_writes_nothing() {
    let f = fixture().await;
    let base = snapshot(&f.service).await;
    let ops = vec![
        kalos_run(None, utc(kl(27, 0, 0))),
        DraftOp::AmendRun {
            run: Target::Created(0),
            to: utc(kl(30, 20, 0)),
        },
        rsvp(&f.runs[0], "1002", RsvpState::Yes),
    ];
    let first = replay(&base, &ops, &policy(), None, &Guild, now()).unwrap();
    let second = replay(&base, &ops, &policy(), None, &Guild, now()).unwrap();
    assert_eq!(first.draft.to_snapshot(), second.draft.to_snapshot());
    let created = first.created[0].clone().unwrap();
    assert!(created.starts_with("preview-"));
    let preview = first.draft.to_snapshot();
    let run = preview.runs.iter().find(|run| run.id == created).unwrap();
    assert_eq!(run.datetime, utc(kl(30, 20, 0)));
    assert_eq!(snapshot(&f.service).await, base, "replay wrote nothing");

    let bad = vec![DraftOp::AmendRun {
        run: Target::Created(0),
        to: utc(kl(30, 20, 0)),
    }];
    let rejected = replay(&base, &bad, &policy(), None, &Guild, now()).unwrap_err();
    assert_eq!(rejected.ord, 0);
}

// ---- rewind -----------------------------------------------------------------

#[tokio::test]
async fn rewinding_the_records_after_a_base_restores_the_base_snapshot() {
    let mut f = fixture().await;
    let base = snapshot(&f.service).await;
    let head = f.service.store().history_head().await.unwrap();
    let upstream = vec![
        amend(&f.runs[0], kl(1, 20, 0)),
        rsvp(&f.runs[0], "1002", RsvpState::No),
        swap(&f.runs[2], &["1002"], &["1003"]),
        fixed_edit(&f.fixed, note("moved")),
        DraftOp::AddFixedRun(NewFixedRun {
            owner_id: "1002".into(),
            channel_id: None,
            bosses: vec!["Kalos".into()],
            ..weekly("", Weekday::Fri, hm(22, 0), &["1002"])
        }),
        status(&f.runs[1], RunStatus::Cancelled),
    ];
    for op in &upstream {
        commit(&mut f.service, op).await;
    }
    let current = snapshot(&f.service).await;
    assert_ne!(current, base);
    let tip = f.service.store().history_head().await.unwrap();
    let records = records_after(&f.service, head.seq).await;
    assert_eq!(records.len(), upstream.len());

    let mut rewound = rewind(&current, &head, &tip, &records).unwrap();
    rewound.revision = base.revision;
    assert_eq!(rewound, base);
    let mut reversed = records.clone();
    reversed.reverse();
    let mut rewound = rewind(&current, &head, &tip, &reversed).unwrap();
    rewound.revision = base.revision;
    assert_eq!(rewound, base, "record order does not matter");

    let draft = vec![rsvp(&f.runs[2], "1001", RsvpState::Yes)];
    let direct = analyze_merge(&base, &current, &draft, &policy(), None, &Guild, now());
    let since = analyze_merge_since(
        &current,
        &head,
        &tip,
        &records,
        &draft,
        &policy(),
        None,
        &Guild,
        now(),
    )
    .unwrap();
    assert!(since.is_clean(), "{:?}", since.conflicts);
    assert_eq!(since.conflicts, direct.conflicts);
    assert_eq!(since.preview, direct.preview);

    // Records committed after `current` was loaded are harmless.
    commit(&mut f.service, &rsvp(&f.runs[2], "1003", RsvpState::Maybe)).await;
    let later = f.service.store().history_head().await.unwrap();
    let more = records_after(&f.service, head.seq).await;
    let mut rewound = rewind(&current, &head, &later, &more).unwrap();
    rewound.revision = base.revision;
    assert_eq!(rewound, base, "a later record changes nothing");
}

/// Every record after `seq`, following each page's cursor.
async fn records_after(service: &Service, seq: u64) -> Vec<ChangeRecord> {
    let mut records = Vec::new();
    let mut cursor = Some(seq);
    while let Some(after) = cursor {
        let mut query = ChangeQuery::new(ChangeFilter::All);
        query.cursor = Some(after);
        query.limit = MAX_PAGE;
        let page = service.store().list_changes(&query).await.unwrap();
        cursor = page.next_cursor;
        records.extend(page.records);
    }
    records
}

#[tokio::test]
async fn rewind_refuses_an_incomplete_or_unlinked_record_list() {
    let mut f = fixture().await;
    let head = f.service.store().history_head().await.unwrap();
    for op in [
        amend(&f.runs[0], kl(1, 20, 0)),
        rsvp(&f.runs[0], "1002", RsvpState::No),
        swap(&f.runs[2], &["1002"], &["1003"]),
    ] {
        commit(&mut f.service, &op).await;
    }
    let current = snapshot(&f.service).await;
    let tip = f.service.store().history_head().await.unwrap();
    let records = records_after(&f.service, head.seq).await;
    assert_eq!(records.len(), 3);
    let refused = |records: &[ChangeRecord]| rewind(&current, &head, &tip, records).unwrap_err();

    let mut missing_middle = records.clone();
    missing_middle.remove(1);
    assert_eq!(
        refused(&missing_middle),
        HistoryGap::Missing {
            expected: head.seq + 2,
            found: Some(head.seq + 3),
        }
    );

    let truncated = &records[..2];
    assert_eq!(
        refused(truncated),
        HistoryGap::Missing {
            expected: head.seq + 3,
            found: None,
        }
    );

    let mut unlinked = records.clone();
    unlinked[1].prev_hash = "0".repeat(64);
    assert_eq!(
        refused(&unlinked),
        HistoryGap::BrokenLink { seq: head.seq + 2 }
    );

    let mut edited = records.clone();
    edited[2].rows.clear();
    assert_eq!(refused(&edited), HistoryGap::Tampered { seq: head.seq + 3 });

    let wrong_base = ChangeRef {
        seq: head.seq,
        hash: "f".repeat(64),
    };
    assert_eq!(
        rewind(&current, &wrong_base, &tip, &records).unwrap_err(),
        HistoryGap::BrokenLink { seq: head.seq + 1 }
    );
    let draft = [rsvp(&f.runs[0], "1001", RsvpState::Yes)];
    assert!(
        analyze_merge_since(
            &current,
            &head,
            &tip,
            &missing_middle,
            &draft,
            &policy(),
            None,
            &Guild,
            now()
        )
        .is_err()
    );
}

// ---- conflict matrix ------------------------------------------------------

#[tokio::test]
async fn conflict_matrix() {
    let f = fixture().await;
    let [r0, _, r2] = f.runs.clone();
    let fixed = f.fixed.clone();
    type Check = Box<dyn Fn(&[MergeConflict]) -> bool>;
    let clean: fn() -> Check = || Box::new(|conflicts| conflicts.is_empty());
    let removed = |entity: Entity, removal: Removal| -> Check {
        Box::new(move |c| {
            c == [MergeConflict::UpstreamRemoved {
                entity: entity.clone(),
                removal,
            }]
        })
    };
    let answer_1002 = || Entity::Rsvp {
        run_id: r0.clone(),
        user_id: "1002".into(),
    };
    let retire = || DraftOp::RetireFixedRun {
        fixed: existing(&fixed),
        weeks: vec![utc(kl(27, 0, 0))],
    };
    let cases: Vec<(&str, Vec<DraftOp>, Vec<DraftOp>, Check)> = vec![
        (
            "both moved a run to different slots",
            vec![amend(&r0, kl(1, 20, 0))],
            vec![amend(&r0, kl(2, 20, 0))],
            {
                let entity = Entity::Run(r0.clone());
                Box::new(move |c| {
                    c.iter().any(|c| matches!(c, MergeConflict::BothChanged { entity: e, field: Field::Slot, .. } if *e == entity))
                })
            },
        ),
        (
            "both moved a run to the same slot",
            vec![amend(&r0, kl(1, 20, 0))],
            vec![amend(&r0, kl(1, 20, 0))],
            clean(),
        ),
        (
            "upstream cancelled the run the draft answers on",
            vec![status(&r0, RunStatus::Cancelled)],
            vec![rsvp(&r0, "1002", RsvpState::Yes)],
            removed(answer_1002(), Removal::Cancelled),
        ),
        (
            "upstream cancelled the run the draft moves",
            vec![status(&r0, RunStatus::Cancelled)],
            vec![amend(&r0, kl(1, 20, 0))],
            removed(Entity::Run(r0.clone()), Removal::Cancelled),
        ),
        (
            "upstream retired the weekly timing the draft edits",
            vec![retire()],
            vec![fixed_edit(&fixed, note("draft"))],
            Box::new(|c| {
                matches!(
                    c,
                    [MergeConflict::OpRejected {
                        ord: 0,
                        on_base: false,
                        ..
                    }]
                )
            }),
        ),
        (
            "both edited the weekly note differently",
            vec![fixed_edit(&fixed, note("upstream"))],
            vec![fixed_edit(&fixed, note("draft"))],
            {
                let fixed = fixed.clone();
                Box::new(move |c| {
                    c.iter().any(|c| matches!(c, MergeConflict::BothChanged { entity: Entity::Fixed(id), field: Field::Note, .. } if *id == fixed))
                })
            },
        ),
        (
            "upstream edited bosses, the draft the note",
            vec![fixed_edit(&fixed, bosses(&["Kalos"]))],
            vec![fixed_edit(&fixed, note("draft"))],
            clean(),
        ),
        (
            "the draft attaches a new run to a timing upstream retired",
            vec![retire()],
            vec![DraftOp::CreateRun {
                fixed: Some(existing(&fixed)),
                channel_id: Some("222".into()),
                // A week with no run of the timing yet.
                week_start: utc(kl(17, 0, 0)),
                datetime: utc(kl(19, 20, 0)),
                bosses: vec!["HFA".into()],
                participants: vec!["1001".into()],
                status: RunStatus::Planned,
                source: RunSource::Amend,
            }],
            Box::new(|c| {
                matches!(
                    c,
                    [MergeConflict::OpRejected {
                        ord: 0,
                        on_base: false,
                        error: ReplayError::Schedule(ScheduleError::UnknownFixedRun(_)),
                    }]
                )
            }),
        ),
        (
            "the draft answers for a member upstream took off the run",
            vec![swap(&r0, &["1002"], &[])],
            vec![rsvp(&r0, "1002", RsvpState::Yes)],
            removed(answer_1002(), Removal::LeftRun),
        ),
        (
            "both answered for one member differently",
            vec![rsvp(&r2, "1002", RsvpState::No)],
            vec![rsvp(&r2, "1002", RsvpState::Yes)],
            Box::new(|c| {
                c.iter().any(|c| {
                    matches!(
                        c,
                        MergeConflict::BothChanged {
                            field: Field::Answer,
                            ..
                        }
                    )
                })
            }),
        ),
    ];
    for (name, upstream, draft, check) in cases {
        let (analysis, _) = merge(&upstream, &draft).await;
        assert!(
            check(&analysis.conflicts),
            "{name}: {:?}",
            analysis.conflicts
        );
        if analysis.conflicts.is_empty() {
            assert!(analysis.is_clean(), "{name}");
        }
    }
}

/// The preview of a merge that must come out clean.
async fn clean_preview(upstream: &[DraftOp], draft: &[DraftOp]) -> ScheduleSnapshot {
    let (analysis, _) = merge(upstream, draft).await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    analysis.preview.unwrap()
}

#[tokio::test]
async fn participants_merge_as_sets() {
    let f = fixture().await;
    let r0 = f.runs[0].clone();
    // Base line-up {1001, 1002}: upstream drops 1002, the draft adds 1003.
    let preview = clean_preview(&[swap(&r0, &["1002"], &[])], &[swap(&r0, &[], &["1003"])]).await;
    assert_eq!(participants(&preview, &r0), set(&["1001", "1003"]));

    // Upstream swaps 1002 for 1004, the draft adds 1003: one union.
    let upstream = [swap(&r0, &["1002"], &["1004"])];
    let preview = clean_preview(&upstream, &[swap(&r0, &[], &["1003"])]).await;
    assert_eq!(participants(&preview, &r0), set(&["1001", "1003", "1004"]));

    // Both drop 1002: the draft's swap no longer applies as written, so the
    // merge refuses it rather than guessing (replay on the current schedule).
    let (analysis, _) = merge(&[swap(&r0, &["1002"], &[])], &[swap(&r0, &["1002"], &[])]).await;
    assert!(
        matches!(
            analysis.conflicts[..],
            [MergeConflict::OpRejected {
                ord: 0,
                on_base: false,
                ..
            }]
        ),
        "{:?}",
        analysis.conflicts
    );

    // The draft re-adds someone upstream removed: set merge keeps the add.
    let upstream = [swap(&r0, &["1001"], &[])];
    let preview = clean_preview(&upstream, &[swap(&r0, &["1002"], &["1003"])]).await;
    let got: Vec<String> = participants(&preview, &r0).into_iter().collect();
    assert_eq!(FieldValue::Set(got), members(&["1003"]));
}

fn party_delta(fixed: &str, add: &[&str], remove: &[&str]) -> DraftOp {
    DraftOp::FixedParticipants {
        fixed: existing(fixed),
        add: owned(add),
        remove: owned(remove),
    }
}

#[tokio::test]
async fn weekly_party_deltas_merge_as_sets() {
    let f = fixture().await;
    let (fixed, r0) = (f.fixed.clone(), f.runs[0].clone());
    let party_of = |state: &ScheduleSnapshot| -> BTreeSet<String> {
        let timing = state.fixed_runs.iter().find(|row| row.id == fixed).unwrap();
        timing.participants.iter().cloned().collect()
    };
    // Upstream adds 1003 to the timing, the draft adds 1004: both apply,
    // on the timing and its live runs.
    let (analysis, current) = merge(
        &[party_delta(&fixed, &["1003"], &[])],
        &[party_delta(&fixed, &["1004"], &[])],
    )
    .await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    assert_eq!(party_of(&current), set(&["1001", "1002", "1003"]));
    let preview = analysis.preview.unwrap();
    assert_eq!(party_of(&preview), set(&["1001", "1002", "1003", "1004"]));
    assert_eq!(
        participants(&preview, &r0),
        set(&["1001", "1002", "1003", "1004"])
    );
    // Upstream substitutes on one run; the draft's weekly join keeps it.
    let upstream = [swap(&r0, &["1002"], &["1004"])];
    let preview = clean_preview(&upstream, &[party_delta(&fixed, &["1003"], &[])]).await;
    assert_eq!(participants(&preview, &r0), set(&["1001", "1003", "1004"]));
    assert_eq!(party_of(&preview), set(&["1001", "1002", "1003"]));
}

#[tokio::test]
async fn per_run_choices_go_stale_when_the_amended_runs_change() {
    let f = fixture().await;
    let [r0, r1, _] = f.runs.clone();
    let edit = DraftOp::ApplyFixedEdit {
        fixed: existing(&f.fixed),
        edit: FixedEdit {
            time: Some(hm(22, 0)),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::PerRun(BTreeMap::from([(
            r1.clone(),
            AmendedRunChoice::KeepForThisWeek,
        )])),
    };
    // Unchanged amended set: clean.
    let (analysis, _) = merge(&[], std::slice::from_ref(&edit)).await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);

    // Upstream amends another run of the timing: the choices no longer cover it.
    let (analysis, _) = merge(&[amend(&r0, kl(1, 20, 0))], std::slice::from_ref(&edit)).await;
    assert!(
        analysis.conflicts.iter().any(|c| matches!(
            c,
            MergeConflict::ChoicesStale { ord: 0, base, current }
                if *base == vec![r1.clone()] && *current == vec![r0.clone(), r1.clone()]
        )),
        "{:?}",
        analysis.conflicts
    );

    // Upstream resets the amended run: it is no longer amended.
    let (analysis, _) = merge(
        &[DraftOp::ResetToFixed { run: existing(&r1) }],
        std::slice::from_ref(&edit),
    )
    .await;
    assert!(
        analysis.conflicts.iter().any(|c| matches!(
            c,
            MergeConflict::ChoicesStale { ord: 0, current, .. } if current.is_empty()
        )),
        "{:?}",
        analysis.conflicts
    );
}

// ---- non-overlapping operations commute --------------------------------------

/// Merge fields of runs, weekly timings and answers, ids kept.
fn merged_rows(state: &ScheduleSnapshot) -> Vec<String> {
    let mut rows: Vec<String> = state
        .runs
        .iter()
        .map(|run| {
            let mut people = run.participants.clone();
            people.sort();
            format!(
                "run {} {:?} {:?} {people:?} {:?} {:?} {:?}",
                run.id, run.datetime, run.bosses, run.channel_id, run.status, run.fixed_run_id
            )
        })
        .chain(state.fixed_runs.iter().map(|row| {
            let mut people = row.participants.clone();
            people.sort();
            format!(
                "fixed {} {} {} {:?} {people:?} {:?} {:?} {}",
                row.id, row.weekday, row.time, row.bosses, row.channel_id, row.note, row.owner_id
            )
        }))
        .chain(state.rsvps.iter().map(|row| {
            format!(
                "rsvp {} {} {:?} {:?}",
                row.run_id, row.user_id, row.state, row.source
            )
        }))
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn non_overlapping_changes_merge_to_applying_both_sides() {
    let f = fixture().await;
    let [r0, r1, r2] = f.runs.clone();
    let fixed = f.fixed.clone();
    let pairs: Vec<(Vec<DraftOp>, Vec<DraftOp>)> = vec![
        (
            vec![amend(&r0, kl(1, 20, 0))],
            vec![amend(&r2, kl(15, 20, 0))],
        ),
        (
            vec![amend(&r0, kl(1, 20, 0))],
            vec![swap(&r0, &[], &["1003"])],
        ),
        (
            vec![rsvp(&r0, "1002", RsvpState::No)],
            vec![rsvp(&r0, "1001", RsvpState::Yes)],
        ),
        (
            vec![swap(&r2, &["1002"], &[])],
            vec![rsvp(&r0, "1003", RsvpState::Maybe)],
        ),
        (
            vec![fixed_edit(&fixed, bosses(&["Kalos"]))],
            vec![fixed_edit(&fixed, note("n"))],
        ),
        (
            vec![status(&r2, RunStatus::Cancelled)],
            vec![amend(&r0, kl(2, 20, 0))],
        ),
        (
            vec![fixed_edit(&fixed, note("n"))],
            vec![DraftOp::ResetToFixed { run: existing(&r1) }],
        ),
        (
            vec![swap(&r0, &["1002"], &["1004"])],
            vec![swap(&r0, &[], &["1003"]), rsvp(&r2, "1001", RsvpState::No)],
        ),
    ];
    for (index, (upstream, draft)) in pairs.iter().enumerate() {
        let (analysis, _) = merge(upstream, draft).await;
        assert!(
            analysis.is_clean(),
            "pair {index}: {:?}",
            analysis.conflicts
        );

        // Upstream then draft, committed for real.
        let mut serial = fixture().await;
        for op in upstream.iter().chain(draft) {
            commit(&mut serial.service, op).await;
        }
        let serial = snapshot(&serial.service).await;
        assert_eq!(
            merged_rows(analysis.preview.as_ref().unwrap()),
            merged_rows(&serial),
            "pair {index}: merge = upstream then draft"
        );
        // And the other way round: the two sides commute.
        let mut swapped = fixture().await;
        for op in draft.iter().chain(upstream) {
            commit(&mut swapped.service, op).await;
        }
        assert_eq!(
            merged_rows(&serial),
            merged_rows(&snapshot(&swapped.service).await),
            "pair {index}: sides commute"
        );
    }
}

#[tokio::test]
async fn created_rows_compare_across_replays_and_notices_are_listed() {
    let f = fixture().await;
    let week = utc(kl(27, 0, 0));
    let draft = vec![
        DraftOp::CreateRun {
            fixed: None,
            channel_id: Some("333".into()),
            week_start: week,
            datetime: utc(kl(29, 20, 0)),
            bosses: vec!["Kalos".into()],
            participants: vec!["1003".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
        DraftOp::SwapParticipants {
            run: Target::Created(0),
            remove: Vec::new(),
            add: vec!["1004".into()],
            via_portal: true,
        },
        status(&f.runs[0], RunStatus::Cancelled),
    ];
    let (analysis, current) = merge(&[amend(&f.runs[2], kl(15, 20, 0))], &draft).await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    let changes = analysis.result_changes.as_ref().unwrap();
    assert!(!changes.changes.is_empty());
    assert!(!analysis.notices_preview.is_empty());
    assert!(analysis.weeks.contains(&week));
    assert_eq!(snapshot(&fixture().await.service).await.runs.len(), 3);
    assert_ne!(analysis.preview.as_ref(), Some(&current));
}

// ---- retirement weeks ---------------------------------------------------------

fn materialised_now(at: DateTime<Utc>) -> Vec<DateTime<Utc>> {
    policy()
        .materialised_weeks(at)
        .unwrap()
        .iter()
        .map(|week| kanade_domain::schedule::utc_instant(week).unwrap())
        .collect()
}

#[tokio::test]
async fn a_retirement_staged_before_a_reset_goes_stale() {
    let mut f = fixture().await;
    let base = snapshot(&f.service).await;
    let staged = materialised_now(now());
    let retire = vec![DraftOp::RetireFixedRun {
        fixed: existing(&f.fixed),
        weeks: staged.clone(),
    }];
    let analysis = analyze_merge(&base, &base, &retire, &policy(), None, &Guild, now());
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);

    // After the next reset the fourth week is materialised with a live run.
    let later = kl(3, 1, 0);
    f.clock.set(later);
    by_tests(&mut f.service)
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let current = snapshot(&f.service).await;
    let fourth = current
        .runs
        .iter()
        .find(|run| run.datetime == utc(kl(21, 21, 30)))
        .expect("the fourth week's run")
        .id
        .clone();
    let analyze =
        |ops: &[DraftOp]| analyze_merge(&base, &current, ops, &policy(), None, &Guild, utc(later));
    let analysis = analyze(&retire);
    let expected = MergeConflict::StaleWeeks {
        ord: 0,
        staged: staged.clone(),
        current: materialised_now(utc(later)),
    };
    assert!(
        analysis.conflicts.contains(&expected),
        "{:?}",
        analysis.conflicts
    );
    assert!(!analysis.is_clean());
    // The run a merge without the conflict would have left live.
    let left = analysis.preview.as_ref().unwrap();
    let run = left.runs.iter().find(|run| run.id == fourth).unwrap();
    assert!(run.status.is_live());

    // Re-staged with the current weeks, the retirement covers it.
    let restaged = vec![DraftOp::RetireFixedRun {
        fixed: existing(&f.fixed),
        weeks: materialised_now(utc(later)),
    }];
    let analysis = analyze(&restaged);
    assert!(
        !analysis
            .conflicts
            .iter()
            .any(|c| matches!(c, MergeConflict::StaleWeeks { .. })),
        "{:?}",
        analysis.conflicts
    );
    let merged = analysis.preview.as_ref().unwrap();
    let run = merged.runs.iter().find(|run| run.id == fourth).unwrap();
    assert_eq!(run.status, RunStatus::Cancelled);
}

// ---- mutation wrappers --------------------------------------------------------

#[tokio::test]
async fn edit_fixed_run_skips_absent_timings_and_empty_edits_before_reading_weeks() {
    let mut f = fixture().await;
    // A UTC instant whose Kuala Lumpur wall clock is past year 9999.
    let edge = Utc.with_ymd_and_hms(9999, 12, 31, 20, 0, 0).unwrap();
    let note = [FixedField::Note];
    let reminders = policy().reminders;
    let mut edit = async |fixed: &str, changed: &[FixedField]| {
        by_tests(&mut f.service)
            .edit_fixed_run(fixed, Default::default(), changed, &[edge], &reminders)
            .await
    };
    let fixed = f.fixed.clone();
    assert_eq!(edit(&fixed, &[]).await.unwrap(), 0);
    assert_eq!(edit("no-such-timing", &note).await.unwrap(), 0);
    // With something to push the week is read, and refused.
    assert!(edit(&fixed, &note).await.is_err());
}
