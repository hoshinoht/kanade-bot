//! Draft operations: the stored codec, replay, rewinding history to a base
//! and the three-way merge into the current schedule.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::{
    drafts::{
        DRAFT_OP_FORMAT, DraftOp, Entity, Field, FieldValue, MergeAnalysis, MergeConflict, Removal,
        ReplayError, Target, analyze_merge, analyze_merge_since, decode, encode, replay,
    },
    history::{
        ChangeFilter, ChangeHistory, ChangeQuery, ChangeRecord, ChangeRef, HistoryGap, MAX_PAGE,
        Origin, rewind,
    },
    ids::IdGenerator,
    members::{Directory, Member},
    schedule::{
        AmendedRunChoice, FixedEdit, FixedEditChoices, FixedEditRequest, NewFixedRun,
        ReminderPolicy, RsvpSource, RsvpState, RunSource, RunStatus, ScheduleError, SchedulePolicy,
        ScheduleSnapshot, StatusChange,
    },
    scheduler::{ScheduleStore, SchedulerService, Scope},
};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::TestClock;

#[derive(Default)]
struct CountingIds(u32);

impl IdGenerator for CountingIds {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{:04}", self.0)
    }
}

struct Guild;

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        ["1001", "1002", "1003", "1004"]
            .contains(&user_id)
            .then(|| Member {
                user_id: user_id.to_owned(),
                display_name: Some(format!("member {user_id}")),
                has_role: true,
                ..Member::default()
            })
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        ["222", "333"].contains(&channel_id)
    }
}

type Service = SchedulerService<MemoryScheduleStore, CountingIds, TestClock>;

fn kl(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
    let month = if d >= 27 { 8 } else { 9 };
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, d, h, mi, 0)
        .unwrap()
}

fn utc(at: DateTime<FixedOffset>) -> DateTime<Utc> {
    at.with_timezone(&Utc)
}

fn now() -> DateTime<Utc> {
    utc(kl(27, 1, 0))
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

/// Weekly Mon 21:30 with runs for 31 Aug, 7 Sep and 14 Sep; the 7 Sep run
/// is amended to Wed 9 Sep 21:00.
struct Fixture {
    service: Service,
    clock: TestClock,
    fixed: String,
    runs: [String; 3],
}

async fn fixture() -> Fixture {
    let clock = TestClock::new(kl(27, 1, 0));
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        CountingIds::default(),
        clock.clone(),
    );
    let fixed = service
        .as_origin(Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into()],
            weekday: Weekday::Mon,
            time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        })
        .await
        .unwrap();
    service
        .as_origin(Origin::for_tests())
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let run_at = |at: DateTime<FixedOffset>| {
        state
            .runs
            .iter()
            .find(|run| run.datetime == utc(at))
            .unwrap()
            .id
            .clone()
    };
    let runs = [
        run_at(kl(31, 21, 30)),
        run_at(kl(7, 21, 30)),
        run_at(kl(14, 21, 30)),
    ];
    service
        .as_origin(Origin::for_tests())
        .amend_run(&runs[1], utc(kl(9, 21, 0)), &policy())
        .await
        .unwrap();
    Fixture {
        service,
        clock,
        fixed,
        runs,
    }
}

async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

fn existing(id: &str) -> Target {
    Target::Existing(id.to_owned())
}

fn id(target: &Target) -> &str {
    match target {
        Target::Existing(id) => id,
        Target::Created(_) => panic!("upstream operations name existing rows"),
    }
}

/// Commit one operation through the service, as another member would.
async fn commit(service: &mut Service, op: &DraftOp) {
    match op {
        DraftOp::AddFixedRun(new) => {
            service
                .as_origin(Origin::for_tests())
                .add_fixed_run(new.clone())
                .await
                .unwrap();
        }
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
            service
                .as_origin(Origin::for_tests())
                .apply_fixed_edit(&request, &Guild, &policy())
                .await
                .unwrap();
        }
        DraftOp::FixedParticipants { fixed, add, remove } => {
            // The real delta operation, as an approved request commits it.
            service
                .as_origin(Origin::for_tests())
                .change_fixed_party(id(fixed), add, remove, &Guild, &policy())
                .await
                .unwrap();
        }
        DraftOp::RetireFixedRun { fixed, weeks } => {
            service
                .as_origin(Origin::for_tests())
                .retire_fixed_run(id(fixed), weeks, &policy().reminders)
                .await
                .unwrap();
        }
        DraftOp::CreateRun { .. } => panic!("not used upstream"),
        DraftOp::AmendRun { run, to } => {
            service
                .as_origin(Origin::for_tests())
                .amend_run(id(run), *to, &policy())
                .await
                .unwrap();
        }
        DraftOp::SetStatus { run, change } => {
            service
                .as_origin(Origin::for_tests())
                .set_status(id(run), *change, &policy().reminders)
                .await
                .unwrap();
        }
        DraftOp::SwapParticipants {
            run,
            remove,
            add,
            via_portal,
        } => {
            service
                .as_origin(Origin::for_tests())
                .swap_participants(id(run), remove, add, *via_portal, &Guild)
                .await
                .unwrap();
        }
        DraftOp::SetRsvp {
            run,
            user_id,
            state,
            source,
        } => {
            service
                .as_origin(Origin::for_tests())
                .set_rsvp(id(run), user_id, *state, *source)
                .await
                .unwrap();
        }
        DraftOp::ResetToFixed { run } => {
            service
                .as_origin(Origin::for_tests())
                .reset_to_fixed(id(run), &policy())
                .await
                .unwrap();
        }
        DraftOp::SetRunBosses { .. }
        | DraftOp::EnsureReminders { .. }
        | DraftOp::RecountRun { .. } => panic!("proposal-only; not used upstream"),
    }
}

/// Analyse `draft` drafted on the fixture after `upstream` was committed.
async fn merge(upstream: &[DraftOp], draft: &[DraftOp]) -> (MergeAnalysis, ScheduleSnapshot) {
    let mut f = fixture().await;
    let base = snapshot(&f.service).await;
    for op in upstream {
        commit(&mut f.service, op).await;
    }
    let current = snapshot(&f.service).await;
    let analysis = analyze_merge(&base, &current, draft, &policy(), &Guild, now());
    (analysis, current)
}

fn amend(run: &str, to: DateTime<FixedOffset>) -> DraftOp {
    DraftOp::AmendRun {
        run: existing(run),
        to: utc(to),
    }
}

fn swap(run: &str, remove: &[&str], add: &[&str]) -> DraftOp {
    DraftOp::SwapParticipants {
        run: existing(run),
        remove: remove.iter().map(|id| (*id).to_owned()).collect(),
        add: add.iter().map(|id| (*id).to_owned()).collect(),
        via_portal: true,
    }
}

fn rsvp(run: &str, user: &str, state: RsvpState) -> DraftOp {
    DraftOp::SetRsvp {
        run: existing(run),
        user_id: user.to_owned(),
        state,
        source: RsvpSource::Chat,
    }
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
        bosses: Some(names.iter().map(|name| (*name).to_owned()).collect()),
        ..FixedEdit::default()
    }
}

fn members(ids: &[&str]) -> FieldValue {
    FieldValue::Set(ids.iter().map(|id| (*id).to_owned()).collect())
}

fn participants(state: &ScheduleSnapshot, run: &str) -> BTreeSet<String> {
    state
        .runs
        .iter()
        .find(|row| row.id == run)
        .unwrap()
        .participants
        .iter()
        .cloned()
        .collect()
}

// ---- codec ----------------------------------------------------------------

fn codec_samples() -> Vec<DraftOp> {
    let at = utc(kl(31, 21, 30));
    vec![
        DraftOp::AddFixedRun(NewFixedRun {
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["HFA".into(), "Kalos".into()],
            weekday: Weekday::Sat,
            time: NaiveTime::from_hms_opt(20, 15, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        }),
        DraftOp::ApplyFixedEdit {
            fixed: Target::Created(0),
            edit: FixedEdit {
                weekday: Some(Weekday::Sun),
                time: Some(NaiveTime::from_hms_opt(21, 0, 0).unwrap()),
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
    ]
}

fn golden_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/v5/vectors/drafts/draft_ops.json")
}

#[test]
fn codec_matches_the_pinned_v1_encoding_and_round_trips() {
    let encoded: Vec<String> = codec_samples()
        .iter()
        .map(|op| encode(op).unwrap())
        .collect();
    if std::env::var_os("KANADE_PRINT_GOLDEN").is_some() {
        let file = serde_json::json!({"format": DRAFT_OP_FORMAT, "ops": encoded});
        println!("{}", serde_json::to_string_pretty(&file).unwrap());
    }
    let golden: serde_json::Value =
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
    let edit = |index: usize, change: &dyn Fn(&mut serde_json::Value)| {
        let mut value: serde_json::Value = serde_json::from_str(&pinned[index]).unwrap();
        change(&mut value);
        decode(&value.to_string())
    };
    // Unknown keys, at the top and nested.
    assert!(
        edit(6, &|v| {
            v["extra"] = 1.into();
        })
        .is_err()
    );
    assert!(
        edit(0, &|v| {
            v["fixed_run"]["extra"] = 1.into();
        })
        .is_err()
    );
    assert!(
        edit(1, &|v| {
            v["edit"]["owner_id"] = "1".into();
        })
        .is_err()
    );
    assert!(
        edit(6, &|v| {
            v["change"]["extra"] = true.into();
        })
        .is_err()
    );
    assert!(
        edit(1, &|v| {
            v["choices"]["all"] = true.into();
        })
        .is_err()
    );
    // Missing optionals must be written as null.
    assert!(
        edit(0, &|v| {
            v["fixed_run"].as_object_mut().unwrap().remove("note");
        })
        .is_err()
    );
    // Only exact HH:MM:SS.
    for bad in [
        "20:15",
        "20:15:00.0",
        "2:15:00",
        "20:15:0x",
        "24:00:00",
        " 20:15:0",
        "+1:15:00",
    ] {
        assert!(
            edit(0, &|v| {
                v["fixed_run"]["time"] = bad.into();
            })
            .is_err(),
            "{bad}"
        );
    }
    // Targets hold exactly one of `existing` or `created`.
    assert!(
        edit(6, &|v| {
            v["run"]["created"] = 0.into();
        })
        .is_err()
    );
    assert!(
        edit(6, &|v| {
            v["run"] = serde_json::json!({});
        })
        .is_err()
    );
    assert!(
        edit(6, &|v| {
            v["run"] = serde_json::json!({"existing": 7});
        })
        .is_err()
    );
    assert!(
        edit(5, &|v| {
            v["run"] = serde_json::json!({"created": -1});
        })
        .is_err()
    );
    // Instants only in the encoder's spelling.
    assert!(
        edit(5, &|v| {
            v["to"] = "2026-09-01T20:00:00+08:00".into();
        })
        .is_err()
    );
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
    let week = utc(kl(27, 0, 0));
    let ops = vec![
        DraftOp::CreateRun {
            fixed: None,
            channel_id: Some("222".into()),
            week_start: week,
            datetime: utc(kl(29, 20, 0)),
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
        DraftOp::AmendRun {
            run: Target::Created(0),
            to: utc(kl(30, 20, 0)),
        },
        rsvp(&f.runs[0], "1002", RsvpState::Yes),
    ];
    let first = replay(&base, &ops, &policy(), &Guild, now()).unwrap();
    let second = replay(&base, &ops, &policy(), &Guild, now()).unwrap();
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
    let rejected = replay(&base, &bad, &policy(), &Guild, now()).unwrap_err();
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
            weekday: Weekday::Fri,
            time: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            participants: vec!["1002".into()],
            note: None,
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
    let direct = analyze_merge(&base, &current, &draft, &policy(), &Guild, now());
    let since = analyze_merge_since(
        &current,
        &head,
        &tip,
        &records,
        &draft,
        &policy(),
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
    let run = |id: &str| Entity::Run(id.to_owned());
    type Check = Box<dyn Fn(&[MergeConflict]) -> bool>;
    let clean: fn() -> Check = || Box::new(|conflicts| conflicts.is_empty());
    let cases: Vec<(&str, Vec<DraftOp>, Vec<DraftOp>, Check)> = vec![
        (
            "both moved a run to different slots",
            vec![amend(&r0, kl(1, 20, 0))],
            vec![amend(&r0, kl(2, 20, 0))],
            {
                let entity = run(&r0);
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
            {
                let entity = Entity::Rsvp {
                    run_id: r0.clone(),
                    user_id: "1002".into(),
                };
                Box::new(move |c| {
                    c == [MergeConflict::UpstreamRemoved {
                        entity: entity.clone(),
                        removal: Removal::Cancelled,
                    }]
                })
            },
        ),
        (
            "upstream cancelled the run the draft moves",
            vec![status(&r0, RunStatus::Cancelled)],
            vec![amend(&r0, kl(1, 20, 0))],
            {
                let entity = run(&r0);
                Box::new(move |c| {
                    c == [MergeConflict::UpstreamRemoved {
                        entity: entity.clone(),
                        removal: Removal::Cancelled,
                    }]
                })
            },
        ),
        (
            "upstream retired the weekly timing the draft edits",
            vec![DraftOp::RetireFixedRun {
                fixed: existing(&fixed),
                weeks: vec![utc(kl(27, 0, 0))],
            }],
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
            vec![DraftOp::RetireFixedRun {
                fixed: existing(&fixed),
                weeks: vec![utc(kl(27, 0, 0))],
            }],
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
            {
                let entity = Entity::Rsvp {
                    run_id: r0.clone(),
                    user_id: "1002".into(),
                };
                Box::new(move |c| {
                    c == [MergeConflict::UpstreamRemoved {
                        entity: entity.clone(),
                        removal: Removal::LeftRun,
                    }]
                })
            },
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

#[tokio::test]
async fn participants_merge_as_sets() {
    let f = fixture().await;
    let r0 = f.runs[0].clone();
    // Base line-up {1001, 1002}: upstream drops 1002, the draft adds 1003.
    let (analysis, _) = merge(&[swap(&r0, &["1002"], &[])], &[swap(&r0, &[], &["1003"])]).await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    let preview = analysis.preview.unwrap();
    assert_eq!(
        participants(&preview, &r0),
        BTreeSet::from(["1001".to_owned(), "1003".to_owned()])
    );

    // Upstream swaps 1002 for 1004, the draft adds 1003: one union.
    let (analysis, _) = merge(
        &[swap(&r0, &["1002"], &["1004"])],
        &[swap(&r0, &[], &["1003"])],
    )
    .await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    assert_eq!(
        participants(&analysis.preview.unwrap(), &r0),
        BTreeSet::from(["1001".to_owned(), "1003".to_owned(), "1004".to_owned()])
    );

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
    let (analysis, _) = merge(
        &[swap(&r0, &["1001"], &[])],
        &[swap(&r0, &["1002"], &["1003"])],
    )
    .await;
    let expected = members(&["1003"]);
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    let preview = analysis.preview.unwrap();
    let got: Vec<String> = participants(&preview, &r0).into_iter().collect();
    assert_eq!(FieldValue::Set(got), expected);
}

fn party_delta(fixed: &str, add: &[&str], remove: &[&str]) -> DraftOp {
    DraftOp::FixedParticipants {
        fixed: existing(fixed),
        add: add.iter().map(|id| (*id).to_owned()).collect(),
        remove: remove.iter().map(|id| (*id).to_owned()).collect(),
    }
}

#[tokio::test]
async fn weekly_party_deltas_merge_as_sets() {
    let f = fixture().await;
    let (fixed, r0) = (f.fixed.clone(), f.runs[0].clone());
    let party_of = |state: &ScheduleSnapshot| -> BTreeSet<String> {
        state
            .fixed_runs
            .iter()
            .find(|row| row.id == fixed)
            .unwrap()
            .participants
            .iter()
            .cloned()
            .collect()
    };
    let set =
        |ids: &[&str]| -> BTreeSet<String> { ids.iter().map(|id| (*id).to_owned()).collect() };
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
    let (analysis, _) = merge(
        &[swap(&r0, &["1002"], &["1004"])],
        &[party_delta(&fixed, &["1003"], &[])],
    )
    .await;
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);
    let preview = analysis.preview.unwrap();
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
            time: Some(NaiveTime::from_hms_opt(22, 0, 0).unwrap()),
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
        .map(|week| kanade::domain::schedule::utc_instant(week).unwrap())
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
    let analysis = analyze_merge(&base, &base, &retire, &policy(), &Guild, now());
    assert!(analysis.is_clean(), "{:?}", analysis.conflicts);

    // After the next reset the fourth week is materialised with a live run.
    let later = kl(3, 1, 0);
    f.clock.set(later);
    f.service
        .as_origin(Origin::for_tests())
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
    let analysis = analyze_merge(&base, &current, &retire, &policy(), &Guild, utc(later));
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
    let analysis = analyze_merge(&base, &current, &restaged, &policy(), &Guild, utc(later));
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
    let note = [kanade::domain::schedule::FixedField::Note];
    let touched = f
        .service
        .as_origin(Origin::for_tests())
        .edit_fixed_run(
            &f.fixed,
            Default::default(),
            &[],
            &[edge],
            &policy().reminders,
        )
        .await
        .unwrap();
    assert_eq!(touched, 0);
    let touched = f
        .service
        .as_origin(Origin::for_tests())
        .edit_fixed_run(
            "no-such-timing",
            Default::default(),
            &note,
            &[edge],
            &policy().reminders,
        )
        .await
        .unwrap();
    assert_eq!(touched, 0);
    // With something to push the week is read, and refused.
    assert!(
        f.service
            .as_origin(Origin::for_tests())
            .edit_fixed_run(
                &f.fixed,
                Default::default(),
                &note,
                &[edge],
                &policy().reminders
            )
            .await
            .is_err()
    );
}
