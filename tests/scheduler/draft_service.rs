//! Administrator drafts over the in-memory store: staging, preview,
//! rebase, merge, expiry and their errors. Nothing here touches the network,
//! and previews never write.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::drafts::{
    DraftEventKind, DraftOp, DraftScope, DraftStatus, DraftStore, ReplayError, Target,
};
use kanade::domain::history::{BlameTarget, ChangeHistory, Origin, Surface, blame};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Directory, Member};
use kanade::domain::notify::{ChannelDirectory, DeliveryJournal, DeliverySettings, plan_notice};
use kanade::domain::schedule::{
    DRAFT_MERGED, FixedEdit, FixedEditChoices, NewFixedRun, NoticeChange, ReminderPolicy,
    RsvpSource, RsvpState, RunSource, RunStatus, SchedulePolicy, ScheduleSnapshot,
};
use kanade::domain::scheduler::{DraftError, ScheduleStore, SchedulerService, Scope, StoreError};
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::common::TestClock;

type Service = SchedulerService<MemoryScheduleStore, RandomIds, TestClock>;

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

struct AnyChannel;

impl ChannelDirectory for AnyChannel {
    fn is_reachable(&self, _channel_id: &str) -> bool {
        true
    }
}

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

/// Two weekly timings (Mon 21:30 in 222, Tue 21:30 in 333) with runs for
/// 31 Aug, 7 Sep and 14 Sep; the 7 Sep run of each is amended.
struct Fixture {
    service: Service,
    fixed: [String; 2],
    runs: [[String; 3]; 2],
}

async fn fixture() -> Fixture {
    // Random ids like production: replay draws must never collide with
    // stored rows, or the owner log renames the wrong rows.
    let mut service = SchedulerService::new(
        MemoryScheduleStore::new(),
        RandomIds,
        TestClock::new(kl(27, 1, 0)),
    );
    let mut fixed = Vec::new();
    for (weekday, channel) in [(Weekday::Mon, "222"), (Weekday::Tue, "333")] {
        fixed.push(
            service
                .as_origin(Origin::for_tests())
                .add_fixed_run(NewFixedRun {
                    owner_id: "1001".into(),
                    channel_id: Some(channel.into()),
                    bosses: vec!["HFA".into()],
                    weekday,
                    time: NaiveTime::from_hms_opt(21, 30, 0).unwrap(),
                    participants: vec!["1001".into(), "1002".into()],
                    note: None,
                })
                .await
                .unwrap(),
        );
    }
    service
        .as_origin(Origin::for_tests())
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let mut runs = [Vec::new(), Vec::new()];
    for (index, fixed_id) in fixed.iter().enumerate() {
        let mut ids: Vec<String> = state
            .runs
            .iter()
            .filter(|run| run.fixed_run_id.as_deref() == Some(fixed_id.as_str()))
            .map(|run| run.id.clone())
            .collect();
        ids.sort_by_key(|id| {
            state
                .runs
                .iter()
                .find(|run| run.id == *id)
                .unwrap()
                .datetime
        });
        runs[index] = ids;
    }
    // Amend each timing's middle run a day later.
    for ids in &runs {
        let to = state
            .runs
            .iter()
            .find(|run| run.id == ids[1])
            .unwrap()
            .datetime
            + chrono::TimeDelta::days(1);
        service
            .as_origin(Origin::for_tests())
            .amend_run(&ids[1], to, &policy())
            .await
            .unwrap();
    }
    Fixture {
        service,
        fixed: [fixed[0].clone(), fixed[1].clone()],
        runs: [
            [runs[0][0].clone(), runs[0][1].clone(), runs[0][2].clone()],
            [runs[1][0].clone(), runs[1][1].clone(), runs[1][2].clone()],
        ],
    }
}

async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

async fn head_seq(service: &Service) -> u64 {
    service.store().history_head().await.unwrap().seq
}

async fn journal_is_empty(service: &Service) {
    assert!(
        service
            .store()
            .load_view()
            .await
            .unwrap()
            .targets()
            .is_empty(),
        "the journal holds no claims"
    );
}

fn existing(id: &str) -> Target {
    Target::Existing(id.to_owned())
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

fn fixed_note(fixed: &str, note: &str) -> DraftOp {
    DraftOp::ApplyFixedEdit {
        fixed: existing(fixed),
        edit: FixedEdit {
            note: Some(note.to_owned()),
            ..FixedEdit::default()
        },
        choices: FixedEditChoices::UpdateAll,
    }
}

async fn stage(service: &mut Service, ops: &[DraftOp]) -> String {
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let mut version = draft.version;
    for op in ops {
        let loaded = service
            .add_draft_op("root", &draft.id, version, op.clone(), &policy(), &Guild)
            .await
            .unwrap();
        version = loaded.draft.version;
    }
    draft.id
}

// ---- staging writes draft rows but not schedule, history or journal --------

#[tokio::test]
async fn staging_and_rebase_write_draft_rows_but_not_schedule_or_journal() {
    let mut f = fixture().await;
    let seq = head_seq(&f.service).await;
    let revision = snapshot(&f.service).await.revision;
    journal_is_empty(&f.service).await;

    let draft = f
        .service
        .create_draft("root", "retime", Some("create-1".into()))
        .await
        .unwrap();
    assert_eq!(draft.version, 1);
    // An exact create retries the draft.
    let replayed = f
        .service
        .create_draft("root", "retime", Some("create-1".into()))
        .await
        .unwrap();
    assert_eq!(replayed.id, draft.id);
    assert!(matches!(
        f.service
            .create_draft("root", "other", Some("create-1".into()))
            .await,
        Err(DraftError::RequestMismatch { .. })
    ));
    assert!(matches!(
        f.service.create_draft("root", "", None).await,
        Err(DraftError::InvalidTitle)
    ));

    let loaded = f
        .service
        .add_draft_op(
            "root",
            &draft.id,
            1,
            amend(&f.runs[0][0], kl(1, 20, 0)),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(loaded.draft.version, 2);
    let loaded = f
        .service
        .edit_draft_op(
            "root",
            &draft.id,
            2,
            0,
            amend(&f.runs[0][0], kl(2, 20, 0)),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(loaded.draft.version, 3);
    // A preview-id target is refused.
    assert!(matches!(
        f.service
            .add_draft_op(
                "root",
                &draft.id,
                3,
                DraftOp::SetRsvp {
                    run: Target::Existing("preview-1".into()),
                    user_id: "1001".into(),
                    state: RsvpState::Yes,
                    source: RsvpSource::Chat,
                },
                &policy(),
                &Guild,
            )
            .await,
        Err(DraftError::ReplayFailed { ord: 1, .. })
    ));
    let preview = f
        .service
        .preview_draft(&draft.id, &policy(), &Guild)
        .await
        .unwrap();
    assert!(preview.is_clean(), "{:?}", preview.conflicts);
    let rebased = f
        .service
        .rebase_draft("root", &draft.id, 3, &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(rebased.version, 4);
    let discarded = f
        .service
        .discard_draft("root", &draft.id, 4, Some("stale".into()))
        .await
        .unwrap();
    assert_eq!(discarded.status, DraftStatus::Discarded);
    assert!(matches!(
        f.service
            .add_draft_op(
                "root",
                &draft.id,
                4,
                amend(&f.runs[0][0], kl(1, 20, 0)),
                &policy(),
                &Guild,
            )
            .await,
        Err(DraftError::Stale { .. })
    ));

    assert_eq!(
        head_seq(&f.service).await,
        seq,
        "no change record was appended"
    );
    assert_eq!(snapshot(&f.service).await.revision, revision);
    journal_is_empty(&f.service).await;
    let events = f.service.store().draft_events(&draft.id).await.unwrap();
    assert_eq!(
        events.iter().map(|event| event.kind).collect::<Vec<_>>(),
        [
            DraftEventKind::Created,
            DraftEventKind::OpAdded,
            DraftEventKind::OpEdited,
            DraftEventKind::Rebased,
            DraftEventKind::Discarded,
        ]
    );
}

// ---- merge ------------------------------------------------------------------

#[tokio::test]
async fn merge_posts_one_notice_per_channel_and_advances_history() {
    let mut f = fixture().await;
    let base_seq = head_seq(&f.service).await;
    let id = stage(
        &mut f.service,
        &[
            amend(&f.runs[0][0], kl(1, 20, 0)),
            swap(&f.runs[1][2], &["1002"], &["1003"]),
        ],
    )
    .await;
    let base = f
        .service
        .store()
        .load_draft(&id)
        .await
        .unwrap()
        .unwrap()
        .draft
        .base;
    let outcome = f
        .service
        .merge_draft("root", &id, 3, &policy(), &Guild, Some("merge-a".into()))
        .await
        .unwrap();
    assert_eq!(outcome.seq, base_seq + 1);
    assert_eq!(outcome.notices.len(), 2, "one summary notice per channel");
    let channels: BTreeSet<_> = outcome
        .notices
        .iter()
        .map(|notice| notice.channel_id.clone())
        .collect();
    assert_eq!(
        channels,
        BTreeSet::from([Some("222".into()), Some("333".into())])
    );
    for notice in &outcome.notices {
        assert_eq!(notice.effect_kind(), DRAFT_MERGED);
        let settings = DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
            attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
        };
        let intent = plan_notice(notice, &Guild, &AnyChannel, settings).expect("plannable");
        assert_eq!(Some(intent.channel_id.clone()), notice.channel_id);
    }
    assert!(!outcome.weeks.is_empty());

    let record = f
        .service
        .store()
        .load_change(outcome.seq)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.surface, Surface::DraftMerge);
    assert_eq!(record.origin.request_id.as_deref(), Some("merge-a"));
    assert_eq!(record.refs, [base]);
    assert!(
        f.service
            .store()
            .verify_history()
            .await
            .unwrap()
            .is_intact(),
        "the merged record verifies"
    );
    // Blame names the merge for the fields it set.
    let blamed = blame(f.service.store(), &BlameTarget::Run(f.runs[0][0].clone()))
        .await
        .unwrap()
        .expect("blamed");
    let slot = blamed
        .lines
        .iter()
        .find(|line| line.field == "slot")
        .expect("slot");
    assert_eq!(slot.last.as_ref().expect("attributed").seq, outcome.seq);
    let loaded = f.service.store().load_draft(&id).await.unwrap().unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Merged);
    assert_eq!(loaded.draft.merged_seq, Some(outcome.seq));
}

#[tokio::test]
async fn merge_notices_name_rsvp_runs_and_both_sides_of_a_move() {
    let mut f = fixture().await;
    // An RSVP-only change names its run's channel with the run listed.
    let id = stage(
        &mut f.service,
        &[rsvp(&f.runs[1][2], "1001", RsvpState::Yes)],
    )
    .await;
    let outcome = f
        .service
        .merge_draft("root", &id, 2, &policy(), &Guild, None)
        .await
        .unwrap();
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.notices[0].channel_id.as_deref(), Some("333"));
    match &outcome.notices[0].change {
        kanade::domain::schedule::NoticeChange::Merged {
            run_ids, fixed_ids, ..
        } => {
            assert_eq!(run_ids, &[f.runs[1][2].clone()]);
            assert!(fixed_ids.is_empty());
        }
        other => panic!("not a merge notice: {other:?}"),
    }

    // Moving a run to another channel notifies both channels.
    let id = stage(
        &mut f.service,
        &[DraftOp::ApplyFixedEdit {
            fixed: Target::Existing(f.fixed[0].clone()),
            edit: FixedEdit {
                channel_id: Some("333".into()),
                ..FixedEdit::default()
            },
            choices: FixedEditChoices::UpdateAll,
        }],
    )
    .await;
    let outcome = f
        .service
        .merge_draft("root", &id, 2, &policy(), &Guild, None)
        .await
        .unwrap();
    let channels: BTreeSet<_> = outcome
        .notices
        .iter()
        .map(|notice| notice.channel_id.clone())
        .collect();
    assert_eq!(
        channels,
        BTreeSet::from([Some("222".into()), Some("333".into())])
    );
    for notice in &outcome.notices {
        match &notice.change {
            kanade::domain::schedule::NoticeChange::Merged { run_ids, .. } => {
                assert!(
                    run_ids.contains(&f.runs[0][0]),
                    "the moved run is named in both summaries: {run_ids:?}"
                );
            }
            other => panic!("not a merge notice: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_merge_that_changes_nothing_is_no_effect() {
    let mut f = fixture().await;
    // A swap that changes nobody is a quiet no-op that still stages.
    let id = stage(&mut f.service, &[swap(&f.runs[0][0], &[], &[])]).await;
    let preview = f
        .service
        .preview_draft(&id, &policy(), &Guild)
        .await
        .unwrap();
    assert!(preview.is_clean(), "{:?}", preview.conflicts);
    let seq = head_seq(&f.service).await;
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::NoEffect)
    ));
    assert_eq!(head_seq(&f.service).await, seq, "nothing was written");
}

#[tokio::test]
async fn request_drafts_are_refused_by_the_admin_methods() {
    let mut f = fixture().await;
    // Member requests arrive through S3; stage one directly at the store.
    let base = f.service.store().history_head().await.unwrap();
    let revision = snapshot(&f.service).await.revision;
    let created = f
        .service
        .store()
        .create_draft(kanade::domain::drafts::NewDraft {
            id: "draft-req".into(),
            kind: kanade::domain::drafts::DraftKind::Request,
            title: "member request".into(),
            author: kanade::domain::history::Actor::member("1001"),
            base,
            base_revision: revision,
            request_type: Some("swap".into()),
            subject: Some(f.runs[0][0].clone()),
            at: now(),
            request: None,
            submit: None,
        })
        .await
        .unwrap();
    let draft = match created {
        kanade::domain::drafts::DraftCreated::Created(draft) => draft,
        _ => panic!("not created"),
    };
    assert!(matches!(
        f.service
            .add_draft_op(
                "root",
                &draft.id,
                1,
                amend(&f.runs[0][0], kl(1, 20, 0)),
                &policy(),
                &Guild,
            )
            .await,
        Err(DraftError::RequestDraft)
    ));
    assert!(matches!(
        f.service.preview_draft(&draft.id, &policy(), &Guild).await,
        Err(DraftError::RequestDraft)
    ));
    assert!(matches!(
        f.service
            .rebase_draft("root", &draft.id, 1, &policy(), &Guild)
            .await,
        Err(DraftError::RequestDraft)
    ));
    assert!(matches!(
        f.service.discard_draft("root", &draft.id, 1, None).await,
        Err(DraftError::RequestDraft)
    ));
    assert!(matches!(
        f.service
            .merge_draft("root", &draft.id, 1, &policy(), &Guild, None)
            .await,
        Err(DraftError::RequestDraft)
    ));
}

#[tokio::test]
async fn a_stale_base_conflicts_and_writes_nothing() {
    let mut f = fixture().await;
    let seq = head_seq(&f.service).await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    // Upstream moves the same run elsewhere.
    f.service
        .as_origin(Origin::for_tests())
        .amend_run(&f.runs[0][0], utc(kl(2, 20, 0)), &policy())
        .await
        .unwrap();
    let preview = f
        .service
        .preview_draft(&id, &policy(), &Guild)
        .await
        .unwrap();
    assert!(!preview.conflicts.is_empty());
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::Conflicts(_))
    ));
    assert_eq!(
        head_seq(&f.service).await,
        seq + 1,
        "only the upstream write landed"
    );
    // A non-overlapping upstream change still merges clean.
    let clean = stage(
        &mut f.service,
        &[rsvp(&f.runs[1][2], "1001", RsvpState::Yes)],
    )
    .await;
    let outcome = f
        .service
        .merge_draft("root", &clean, 2, &policy(), &Guild, None)
        .await
        .unwrap();
    assert_eq!(outcome.seq, seq + 2);
}

#[tokio::test]
async fn a_second_merger_gets_already_merged_and_a_retry_replays() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[fixed_note(&f.fixed[0], "draft")]).await;
    let first = f
        .service
        .merge_draft("root", &id, 2, &policy(), &Guild, Some("merge-a".into()))
        .await
        .unwrap();
    assert!(matches!(
        f.service.merge_draft("root", &id, 3, &policy(), &Guild, Some("merge-b".into())).await,
        Err(DraftError::AlreadyMerged { seq }) if seq == first.seq
    ));
    // The exact retry (same arguments) replays instead.
    assert!(matches!(
        f.service.merge_draft("root", &id, 2, &policy(), &Guild, Some("merge-a".into())).await,
        Err(DraftError::AlreadyApplied { seq, .. }) if seq == first.seq
    ));
    // A reused merge id for another draft is a mismatch.
    let other = stage(&mut f.service, &[fixed_note(&f.fixed[1], "other")]).await;
    assert!(matches!(
        f.service.merge_draft("root", &other, 2, &policy(), &Guild, Some("merge-a".into())).await,
        Err(DraftError::IdempotencyMismatch { seq }) if seq == first.seq
    ));
    // A new request id for the merged draft reports the merge, not a replay.
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 3, &policy(), &Guild, None)
            .await,
        Err(DraftError::AlreadyMerged { seq }) if seq == first.seq
    ));
}

#[tokio::test]
async fn a_history_gap_is_refused() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    f.service
        .as_origin(Origin::for_tests())
        .set_rsvp(&f.runs[1][2], "1001", RsvpState::No, RsvpSource::Chat)
        .await
        .unwrap();
    let tip = f.service.store().history_head().await.unwrap();
    f.service.store().tamper_change(tip.seq, |record| {
        record.notices.push("forged".into());
    });
    assert!(matches!(
        f.service.preview_draft(&id, &policy(), &Guild).await,
        Err(DraftError::HistoryGap(_))
    ));
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::HistoryGap(_))
    ));
}

// ---- op edits -----------------------------------------------------------------

#[tokio::test]
async fn removing_an_operation_renumbers_or_refuses() {
    let mut f = fixture().await;
    // [add timing, amend its run, answer on it]: removing the creation
    // leaves the amendment dangling.
    let draft = f.service.create_draft("root", "chain", None).await.unwrap();
    let added = NewFixedRun {
        owner_id: "1001".into(),
        channel_id: Some("222".into()),
        bosses: vec!["Kalos".into()],
        weekday: Weekday::Fri,
        time: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
        participants: vec!["1001".into()],
        note: None,
    };
    let week = utc(kl(27, 0, 0));
    let mut version = draft.version;
    for op in [
        DraftOp::AddFixedRun(added),
        DraftOp::CreateRun {
            fixed: Some(Target::Created(0)),
            channel_id: Some("222".into()),
            week_start: week,
            datetime: utc(kl(29, 20, 0)),
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
        DraftOp::SetRsvp {
            run: Target::Created(1),
            user_id: "1001".into(),
            state: RsvpState::Yes,
            source: RsvpSource::Chat,
        },
    ] {
        version = f
            .service
            .add_draft_op("root", &draft.id, version, op, &policy(), &Guild)
            .await
            .unwrap()
            .draft
            .version;
    }
    assert!(matches!(
        f.service
            .remove_draft_op("root", &draft.id, version, 0, &policy(), &Guild)
            .await,
        Err(DraftError::EditRefused(_))
    ));
    // [amend an existing run, add a timing, attach to it]: removing the
    // amendment renumbers the attachment from 1 to 0.
    let draft = f.service.create_draft("root", "chain", None).await.unwrap();
    let mut version = draft.version;
    let week = utc(kl(27, 0, 0));
    for op in [
        amend(&f.runs[0][0], kl(1, 20, 0)),
        DraftOp::AddFixedRun(NewFixedRun {
            owner_id: "1001".into(),
            channel_id: Some("222".into()),
            bosses: vec!["Kalos".into()],
            weekday: Weekday::Fri,
            time: NaiveTime::from_hms_opt(22, 0, 0).unwrap(),
            participants: vec!["1001".into()],
            note: None,
        }),
        DraftOp::CreateRun {
            fixed: Some(Target::Created(1)),
            channel_id: Some("222".into()),
            week_start: week,
            datetime: utc(kl(29, 20, 0)),
            bosses: vec!["Kalos".into()],
            participants: vec!["1001".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        },
    ] {
        version = f
            .service
            .add_draft_op("root", &draft.id, version, op, &policy(), &Guild)
            .await
            .unwrap()
            .draft
            .version;
    }
    let loaded = f
        .service
        .remove_draft_op("root", &draft.id, version, 0, &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(loaded.ops.len(), 2);
    assert!(matches!(
        loaded.ops[1].op,
        DraftOp::CreateRun {
            fixed: Some(Target::Created(0)),
            ..
        }
    ));
    // Editing the creation into a non-creating op is refused as well.
    assert!(matches!(
        f.service
            .edit_draft_op(
                "root",
                &draft.id,
                loaded.draft.version,
                0,
                swap(&f.runs[0][2], &[], &["1003"]),
                &policy(),
                &Guild,
            )
            .await,
        Err(DraftError::EditRefused(_))
    ));
    assert!(matches!(
        f.service
            .remove_draft_op(
                "root",
                &draft.id,
                loaded.draft.version,
                9,
                &policy(),
                &Guild
            )
            .await,
        Err(DraftError::EditRefused(_))
    ));
}

// ---- rebase -------------------------------------------------------------------

#[tokio::test]
async fn rebase_is_refused_while_conflicted_then_succeeds() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    f.service
        .as_origin(Origin::for_tests())
        .amend_run(&f.runs[0][0], utc(kl(2, 20, 0)), &policy())
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .rebase_draft("root", &id, 2, &policy(), &Guild)
            .await,
        Err(DraftError::Conflicts(_))
    ));
    // Drop the conflicting operation, then rebase onto the head.
    let loaded = f
        .service
        .remove_draft_op("root", &id, 2, 0, &policy(), &Guild)
        .await
        .unwrap();
    assert!(loaded.ops.is_empty());
    let rebased = f
        .service
        .rebase_draft("root", &id, 3, &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(rebased.version, 4);
    assert_eq!(
        rebased.base,
        f.service.store().history_head().await.unwrap()
    );
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 4, &policy(), &Guild, None)
            .await,
        Err(DraftError::Empty)
    ));
}

// ---- expiry -------------------------------------------------------------------

async fn scope_of(service: &Service, id: &str) -> DraftScope {
    service
        .store()
        .load_draft(id)
        .await
        .unwrap()
        .unwrap()
        .draft
        .scope
}

#[tokio::test]
async fn the_scope_is_derived_from_the_staged_run_ops() {
    let mut f = fixture().await;
    let draft = f
        .service
        .create_draft("root", "retime", None)
        .await
        .unwrap();
    assert_eq!(
        scope_of(&f.service, &draft.id).await,
        DraftScope::Weekly,
        "a fresh draft has no expiry"
    );
    let week = snapshot(&f.service)
        .await
        .runs
        .iter()
        .find(|run| run.id == f.runs[0][0])
        .unwrap()
        .week_start;
    // Staging a run op sets the earliest touched week.
    let loaded = f
        .service
        .add_draft_op(
            "root",
            &draft.id,
            1,
            amend(&f.runs[0][0], kl(1, 20, 0)),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Week(week));
    // A fixed-only op leaves the scope alone.
    let loaded = f
        .service
        .add_draft_op(
            "root",
            &draft.id,
            2,
            fixed_note(&f.fixed[0], "prog"),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Week(week));
    // Removing the only run op clears the expiry.
    let loaded = f
        .service
        .remove_draft_op("root", &draft.id, 3, 0, &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Weekly);
    // The fixed-only draft merges after any number of resets.
    f.service.clock().set(kl(20, 1, 0));
    let outcome = f
        .service
        .merge_draft(
            "root",
            &draft.id,
            loaded.draft.version,
            &policy(),
            &Guild,
            None,
        )
        .await
        .unwrap();
    // The merge also commits the weeks materialised since, but only the
    // edited timing's channel hears about it: the other timing's new runs
    // are routine and quiet.
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.notices[0].channel_id.as_deref(), Some("222"));
    let state = snapshot(&f.service).await;
    let timing_of = |id: &str| {
        state
            .runs
            .iter()
            .find(|run| run.id == id)
            .and_then(|run| run.fixed_run_id.clone())
    };
    match &outcome.notices[0].change {
        NoticeChange::Merged {
            run_ids, fixed_ids, ..
        } => {
            assert_eq!(fixed_ids, &[f.fixed[0].clone()]);
            // Like a plain timing edit, the edited timing's channel is told;
            // its newly materialised runs are listed with it.
            assert!(
                run_ids.iter().any(
                    |id| !f.runs[0].contains(id) && timing_of(id).as_ref() == Some(&f.fixed[0])
                ),
                "{run_ids:?}"
            );
            assert!(
                run_ids
                    .iter()
                    .all(|id| timing_of(id).as_ref() == Some(&f.fixed[0])),
                "{run_ids:?}"
            );
        }
        other => panic!("not a merge notice: {other:?}"),
    }
}

#[tokio::test]
async fn edits_and_rebases_re_derive_the_scope() {
    let mut f = fixture().await;
    let week_of = |state: &ScheduleSnapshot, id: &str| {
        state
            .runs
            .iter()
            .find(|run| run.id == id)
            .unwrap()
            .week_start
    };
    let state = snapshot(&f.service).await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    assert_eq!(
        scope_of(&f.service, &id).await,
        DraftScope::Week(week_of(&state, &f.runs[0][0]))
    );
    // Editing the only run op onto a later week's run moves the expiry.
    let edited = f
        .service
        .edit_draft_op(
            "root",
            &id,
            2,
            0,
            rsvp(&f.runs[1][2], "1001", RsvpState::Yes),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(
        edited.draft.scope,
        DraftScope::Week(week_of(&state, &f.runs[1][2]))
    );
    // Upstream moves that run a week later; rebasing re-derives on the head.
    f.service
        .as_origin(Origin::for_tests())
        .amend_run(&f.runs[1][2], utc(kl(22, 21, 30)), &policy())
        .await
        .unwrap();
    let moved = snapshot(&f.service).await;
    assert!(week_of(&moved, &f.runs[1][2]) > week_of(&state, &f.runs[1][2]));
    let rebased = f
        .service
        .rebase_draft("root", &id, 3, &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(
        rebased.scope,
        DraftScope::Week(week_of(&moved, &f.runs[1][2]))
    );
}

#[tokio::test]
async fn a_staged_run_must_sit_in_its_slots_week() {
    let mut f = fixture().await;
    let draft = f
        .service
        .create_draft("root", "new run", None)
        .await
        .unwrap();
    let op = |week_start: DateTime<Utc>| DraftOp::CreateRun {
        fixed: None,
        channel_id: Some("222".into()),
        week_start,
        datetime: utc(kl(29, 20, 0)),
        bosses: vec!["Kalos".into()],
        participants: vec!["1001".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
    };
    let right = utc(kl(27, 0, 0));
    assert!(matches!(
        f.service
            .add_draft_op(
                "root",
                &draft.id,
                1,
                op(right + chrono::TimeDelta::days(7)),
                &policy(),
                &Guild,
            )
            .await,
        Err(DraftError::ReplayFailed {
            ord: 0,
            error: ReplayError::WeekMismatch { week_start, slot_week },
        }) if slot_week == right && week_start == right + chrono::TimeDelta::days(7)
    ));
    let loaded = f
        .service
        .add_draft_op("root", &draft.id, 1, op(right), &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(loaded.draft.scope, DraftScope::Week(right));
}

#[tokio::test]
async fn a_run_op_draft_expires_after_the_reset_and_cannot_merge() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    // Still its week: the preview is clean.
    let preview = f
        .service
        .preview_draft(&id, &policy(), &Guild)
        .await
        .unwrap();
    assert!(preview.is_clean(), "{:?}", preview.conflicts);
    // Past the reset the tick expiry closes it.
    f.service.clock().set(kl(3, 1, 0));
    let expired = f.service.expire_due_drafts(&policy()).await.unwrap();
    assert_eq!(expired.ids, std::slice::from_ref(&id));
    assert!(expired.notices.is_empty(), "an admin draft notifies nobody");
    let loaded = f.service.store().load_draft(&id).await.unwrap().unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    assert_eq!(loaded.draft.version, 2, "expiry does not bump the version");
    assert!(matches!(
        f.service.preview_draft(&id, &policy(), &Guild).await,
        Err(DraftError::Expired)
    ));
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::Expired)
    ));
}

#[tokio::test]
async fn merge_marks_an_expired_draft_when_the_tick_did_not() {
    let mut f = fixture().await;
    let id = stage(&mut f.service, &[amend(&f.runs[0][0], kl(1, 20, 0))]).await;
    // Straight past the reset without expiring first.
    f.service.clock().set(kl(3, 1, 0));
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::Expired)
    ));
    let loaded = f.service.store().load_draft(&id).await.unwrap().unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    // Closed by the system expiry actor, as the tick would.
    let system = kanade::domain::history::Actor::system("delivery");
    assert_eq!(loaded.draft.closed_by.as_ref(), Some(&system));
    let events = f.service.store().draft_events(&id).await.unwrap();
    let last = events.last().unwrap();
    assert_eq!(last.kind, DraftEventKind::Expired);
    assert_eq!(last.actor, system);
    // Idempotent: merging again reports expiry, not staleness.
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::Expired)
    ));
    // So does previewing.
    assert!(matches!(
        f.service.preview_draft(&id, &policy(), &Guild).await,
        Err(DraftError::Expired)
    ));
}

#[tokio::test]
async fn an_upstream_move_into_a_past_week_expires_the_draft_at_merge() {
    let mut f = fixture().await;
    let id = stage(
        &mut f.service,
        &[rsvp(&f.runs[1][2], "1001", RsvpState::Yes)],
    )
    .await;
    // The stored scope is the run's week; upstream then moves the run into
    // last week (26 Aug 20:00 +08:00, before the 27 Aug reset).
    let past = FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, 8, 26, 20, 0, 0)
        .unwrap();
    f.service
        .as_origin(Origin::for_tests())
        .amend_run(&f.runs[1][2], utc(past), &policy())
        .await
        .unwrap();
    let seq = head_seq(&f.service).await;
    assert!(matches!(
        scope_of(&f.service, &id).await,
        DraftScope::Week(week) if week > utc(past)
    ));
    assert!(matches!(
        f.service.preview_draft(&id, &policy(), &Guild).await,
        Err(DraftError::Expired)
    ));
    assert!(matches!(
        f.service
            .merge_draft("root", &id, 2, &policy(), &Guild, None)
            .await,
        Err(DraftError::Expired)
    ));
    assert_eq!(head_seq(&f.service).await, seq, "nothing was merged");
    let loaded = f.service.store().load_draft(&id).await.unwrap().unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    assert_eq!(
        loaded.draft.closed_by,
        Some(kanade::domain::history::Actor::system("delivery"))
    );
}

// ---- revision race --------------------------------------------------------------

/// A store failing the first merge commit with a revision conflict.
struct Racy<'a> {
    inner: &'a MemoryScheduleStore,
    attempts: AtomicUsize,
    race: Race,
}

enum Race {
    /// A bare conflict: the retry re-analyses the same schedule and commits.
    FakeConflict,
    /// A conflicting upstream commit lands first: the retry re-analyses and
    /// reports the conflict instead of committing.
    RealConflict { run: String, to: DateTime<Utc> },
    /// The tick expires the draft at the reset edge, between the merge's
    /// expiry check and its commit.
    Expire,
}

impl ScheduleStore for Racy<'_> {
    async fn load(&self, scope: &Scope) -> Result<ScheduleSnapshot, StoreError> {
        self.inner.load(scope).await
    }

    async fn recorded_request(
        &self,
        actor: &kanade::domain::history::Actor,
        request_id: &str,
    ) -> Result<Option<kanade::domain::scheduler::RecordedRequest>, StoreError> {
        self.inner.recorded_request(actor, request_id).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: kanade::domain::schedule::ChangeSet,
        meta: kanade::domain::history::ChangeMeta,
    ) -> Result<Option<kanade::domain::scheduler::Committed>, StoreError> {
        self.inner.commit(expected_revision, changes, meta).await
    }
}

impl ChangeHistory for Racy<'_> {
    async fn load_change(
        &self,
        seq: u64,
    ) -> Result<Option<kanade::domain::history::ChangeRecord>, StoreError> {
        self.inner.load_change(seq).await
    }

    async fn load_checked(
        &self,
        seq: u64,
    ) -> Result<kanade::domain::history::CheckedChange, StoreError> {
        self.inner.load_checked(seq).await
    }

    async fn list_changes(
        &self,
        query: &kanade::domain::history::ChangeQuery,
    ) -> Result<kanade::domain::history::ChangePage, StoreError> {
        self.inner.list_changes(query).await
    }

    async fn verify_history(
        &self,
    ) -> Result<kanade::domain::history::HistoryVerification, StoreError> {
        self.inner.verify_history().await
    }

    async fn history_head(&self) -> Result<kanade::domain::history::ChangeRef, StoreError> {
        self.inner.history_head().await
    }
}

impl DraftStore for Racy<'_> {
    async fn snapshot_with_head(
        &self,
    ) -> Result<(ScheduleSnapshot, kanade::domain::history::ChangeRef), StoreError> {
        self.inner.snapshot_with_head().await
    }

    async fn records_after(
        &self,
        base: &kanade::domain::history::ChangeRef,
    ) -> Result<Vec<kanade::domain::history::ChangeRecord>, StoreError> {
        self.inner.records_after(base).await
    }

    async fn create_draft(
        &self,
        new: kanade::domain::drafts::NewDraft,
    ) -> Result<kanade::domain::drafts::DraftCreated, StoreError> {
        self.inner.create_draft(new).await
    }

    async fn load_draft(
        &self,
        id: &str,
    ) -> Result<Option<kanade::domain::drafts::LoadedDraft>, StoreError> {
        self.inner.load_draft(id).await
    }

    async fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> Result<Vec<kanade::domain::drafts::StoredDraft>, StoreError> {
        self.inner.list_drafts(status).await
    }

    async fn recorded_draft_request(
        &self,
        author: &kanade::domain::history::Actor,
        request_id: &str,
    ) -> Result<Option<(String, kanade::domain::drafts::StoredDraft)>, StoreError> {
        self.inner.recorded_draft_request(author, request_id).await
    }

    async fn draft_events(
        &self,
        id: &str,
    ) -> Result<Vec<kanade::domain::drafts::DraftEvent>, StoreError> {
        self.inner.draft_events(id).await
    }

    async fn update_draft(
        &self,
        update: kanade::domain::drafts::DraftUpdate,
    ) -> Result<kanade::domain::drafts::DraftWrite, StoreError> {
        self.inner.update_draft(update).await
    }

    async fn commit_merge(
        &self,
        expected_revision: u64,
        changes: kanade::domain::schedule::ChangeSet,
        meta: kanade::domain::history::ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> Result<kanade::domain::drafts::MergeCommit, StoreError> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            match &self.race {
                Race::FakeConflict => {
                    return Err(StoreError::Conflict {
                        expected: expected_revision,
                        found: expected_revision + 1,
                    });
                }
                Race::RealConflict { run, to } => {
                    // A conflicting upstream commit lands between the merge's
                    // analysis and its commit.
                    let snapshot = self.inner.load(&Scope::All).await?;
                    let mut moved = snapshot
                        .runs
                        .iter()
                        .find(|row| row.id == *run)
                        .expect("run")
                        .clone();
                    moved.datetime = *to;
                    let upstream = kanade::domain::history::ChangeMeta {
                        origin: kanade::domain::history::Origin::new(
                            kanade::domain::history::Actor::member("2"),
                            Surface::Discord,
                        ),
                        at: now(),
                        notices: Vec::new(),
                        refs: Vec::new(),
                        request_digest: None,
                        expect: Default::default(),
                    };
                    self.inner
                        .commit(
                            snapshot.revision,
                            kanade::domain::schedule::ChangeSet {
                                changes: vec![kanade::domain::schedule::Change::PutRun(moved)],
                            },
                            upstream,
                        )
                        .await
                        .expect("upstream commit");
                }
                Race::Expire => {
                    let far = now() + chrono::TimeDelta::days(365);
                    self.inner
                        .expire_drafts(
                            far,
                            now(),
                            &kanade::domain::history::Actor::system("delivery"),
                        )
                        .await
                        .expect("expire");
                }
            }
        }
        self.inner
            .commit_merge(
                expected_revision,
                changes,
                meta,
                draft_id,
                expected_version,
                note,
            )
            .await
    }

    async fn expire_drafts(
        &self,
        week: DateTime<Utc>,
        at: DateTime<Utc>,
        actor: &kanade::domain::history::Actor,
    ) -> Result<Vec<String>, StoreError> {
        self.inner.expire_drafts(week, at, actor).await
    }
}

#[tokio::test]
async fn a_revision_race_is_re_analysed_and_retried() {
    let f = fixture().await;
    let store = f.service.store();
    let racy = Racy {
        inner: store,
        attempts: AtomicUsize::new(0),
        race: Race::FakeConflict,
    };
    let mut service: SchedulerService<Racy<'_>, RandomIds, TestClock> =
        SchedulerService::new(racy, RandomIds, TestClock::new(kl(27, 1, 0)));
    let base = service.store().history_head().await.unwrap();
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    assert_eq!(draft.base, base);
    let loaded = service
        .add_draft_op(
            "root",
            &draft.id,
            1,
            amend(&f.runs[0][0], kl(1, 20, 0)),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    let outcome = service
        .merge_draft(
            "root",
            &draft.id,
            loaded.draft.version,
            &policy(),
            &Guild,
            None,
        )
        .await
        .unwrap();
    assert_eq!(outcome.notices.len(), 1);
    assert_eq!(outcome.seq, base.seq + 1);
    // One failed commit plus the retry.
    assert_eq!(service.store().attempts.load(Ordering::SeqCst), 2);
    let tip = f.service.store().history_head().await.unwrap();
    assert_eq!(tip.seq, outcome.seq);
}

#[tokio::test]
async fn a_conflicting_race_is_re_analysed_into_conflicts() {
    let f = fixture().await;
    let store = f.service.store();
    let racy = Racy {
        inner: store,
        attempts: AtomicUsize::new(0),
        race: Race::RealConflict {
            run: f.runs[0][0].clone(),
            to: utc(kl(2, 20, 0)),
        },
    };
    let mut service: SchedulerService<Racy<'_>, RandomIds, TestClock> =
        SchedulerService::new(racy, RandomIds, TestClock::new(kl(27, 1, 0)));
    let base = service.store().history_head().await.unwrap();
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let loaded = service
        .add_draft_op(
            "root",
            &draft.id,
            1,
            amend(&f.runs[0][0], kl(1, 20, 0)),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    // The first commit raced a real conflicting upstream move; the retry
    // re-analysed and reported the conflict instead of committing.
    assert!(matches!(
        service
            .merge_draft(
                "root",
                &draft.id,
                loaded.draft.version,
                &policy(),
                &Guild,
                None,
            )
            .await,
        Err(DraftError::Conflicts(conflicts))
            if conflicts.iter().any(|conflict| matches!(
                conflict,
                kanade::domain::drafts::MergeConflict::BothChanged {
                    field: kanade::domain::drafts::Field::Slot,
                    ..
                }
            ))
    ));
    // The merge committed nothing itself; only the upstream commit landed.
    assert_eq!(service.store().attempts.load(Ordering::SeqCst), 1);
    let tip = f.service.store().history_head().await.unwrap();
    assert_eq!(tip.seq, base.seq + 1, "only the upstream commit landed");
}

#[tokio::test]
async fn a_draft_expired_between_check_and_commit_reports_expired() {
    let f = fixture().await;
    let racy = Racy {
        inner: f.service.store(),
        attempts: AtomicUsize::new(0),
        race: Race::Expire,
    };
    let mut service: SchedulerService<Racy<'_>, RandomIds, TestClock> =
        SchedulerService::new(racy, RandomIds, TestClock::new(kl(27, 1, 0)));
    let base = service.store().history_head().await.unwrap();
    let draft = service.create_draft("root", "retime", None).await.unwrap();
    let loaded = service
        .add_draft_op(
            "root",
            &draft.id,
            1,
            amend(&f.runs[0][0], kl(1, 20, 0)),
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert!(matches!(
        service
            .merge_draft(
                "root",
                &draft.id,
                loaded.draft.version,
                &policy(),
                &Guild,
                None,
            )
            .await,
        Err(DraftError::Expired)
    ));
    assert_eq!(service.store().attempts.load(Ordering::SeqCst), 1);
    let tip = f.service.store().history_head().await.unwrap();
    assert_eq!(tip, base, "nothing was merged");
}
