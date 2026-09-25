//! Member requests over the in-memory store: submit, limits, withdraw,
//! admin edit, approve (merge) and reject, expiry and requester notices.

use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::drafts::{DraftStatus, DraftStore, RequestLimit};
use kanade::domain::history::{Actor, BlameTarget, ChangeHistory, Origin, Surface, Via, blame};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Directory, Member};
use kanade::domain::notify::{ChannelDirectory, DeliverySettings, plan_notice};
use kanade::domain::requests::{MemberGate, NoFreezes, RequestRefusal, RequestSpec, Subject};
use kanade::domain::schedule::{
    AmendedRunChoice, FixedEdit, FixedEditChoices, NewFixedRun, NoticeChange, ReminderPolicy,
    RequestDecision, SchedulePolicy, ScheduleSnapshot,
};
use kanade::domain::scheduler::{DraftError, RequestError, ScheduleStore, SchedulerService, Scope};
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

struct Frozen(&'static str);

impl MemberGate for Frozen {
    fn is_frozen(&self, user_id: &str) -> bool {
        user_id == self.0
    }
}

fn kl(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
    let month = if d >= 27 { 8 } else { 9 };
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, d, h, mi, 0)
        .unwrap()
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

fn admin() -> Actor {
    Actor::admin("root")
}

/// Two weekly timings (Mon 21:30 in 222, Tue 21:30 in 333; party 1001 and
/// 1002) with three weeks of runs; each timing's middle run is amended.
struct Fixture {
    service: Service,
    fixed: [String; 2],
    runs: [[String; 3]; 2],
}

async fn fixture() -> Fixture {
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
        let mut rows: Vec<_> = state
            .runs
            .iter()
            .filter(|run| run.fixed_run_id.as_deref() == Some(fixed_id.as_str()))
            .collect();
        rows.sort_by_key(|run| run.datetime);
        runs[index] = rows.iter().map(|run| run.id.clone()).collect();
    }
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

async fn submit(
    service: &mut Service,
    member: &str,
    spec: RequestSpec,
) -> Result<kanade::domain::drafts::StoredDraft, RequestError> {
    service
        .submit_request(member, "please", spec, None, &policy(), &Guild, &NoFreezes)
        .await
}

/// Submit as 1003 with the request id `sub-1`.
async fn once_as(
    service: &mut Service,
    spec: RequestSpec,
) -> Result<kanade::domain::drafts::StoredDraft, RequestError> {
    service
        .submit_request(
            "1003",
            "please",
            spec,
            Some("sub-1".into()),
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
}

fn join(run: &str) -> RequestSpec {
    RequestSpec::Join(Subject::Run(run.to_owned()))
}

#[tokio::test]
async fn an_approved_request_merges_as_the_admin_and_tells_only_the_requester() {
    let mut f = fixture().await;
    let request = submit(&mut f.service, "1003", join(&f.runs[0][0]))
        .await
        .unwrap();
    assert_eq!(request.status, DraftStatus::Submitted);
    assert_eq!(request.request_type.as_deref(), Some("join"));
    let base = request.base.clone();
    let approved = f
        .service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let run = snapshot(&f.service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == f.runs[0][0])
        .unwrap();
    assert!(run.participants.contains(&"1003".to_owned()));
    // One RequestMerge record linked to the request and its base.
    let seq = approved.merge.seq;
    let record = f.service.store().load_change(seq).await.unwrap().unwrap();
    assert_eq!(record.origin.surface, Surface::RequestMerge);
    assert_eq!(record.origin.actor, admin());
    assert_eq!(
        record.origin.request_id,
        Some(format!("merge:{}@v1", request.id))
    );
    assert_eq!(record.refs, std::slice::from_ref(&base));
    let stored = f
        .service
        .store()
        .load_draft(&request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.draft.status, DraftStatus::Merged);
    assert_eq!(stored.draft.merged_seq, Some(seq));
    // Blame names the admin through the request merge.
    let blamed = blame(f.service.store(), &BlameTarget::Run(f.runs[0][0].clone()))
        .await
        .unwrap()
        .unwrap();
    let party = blamed
        .lines
        .iter()
        .find(|line| line.field == "participants")
        .and_then(|line| line.last.clone())
        .unwrap();
    assert_eq!(party.seq, seq);
    assert_eq!(party.actor, admin());
    assert_eq!(party.surface, Surface::RequestMerge);
    assert_eq!(party.via, Via::Referenced(vec![base]));
    // The requester notice lists and mentions only the requester.
    let notice = &approved.requester_notice;
    assert_eq!(notice.listed, ["1003"]);
    assert_eq!(notice.channel_id.as_deref(), Some("222"));
    assert!(matches!(
        &notice.change,
        NoticeChange::RequestDecided { decision: RequestDecision::Approved, request, .. }
            if *request == stored.draft.id
    ));
    assert_eq!(
        notice.effect_context(),
        [format!("request:{}:approved", request.id)]
    );
    let intent = plan_notice(
        notice,
        &Guild,
        &AnyChannel,
        DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
            attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
        },
    )
    .unwrap();
    assert!(
        intent.mentions.iter().all(|user| user == "1003"),
        "{:?}",
        intent.mentions
    );
}

#[tokio::test]
async fn a_requester_who_left_the_run_is_refused_at_approval() {
    let mut f = fixture().await;
    let swap = RequestSpec::Swap {
        subject: Subject::Run(f.runs[0][0].clone()),
        with: "1003".into(),
    };
    let request = submit(&mut f.service, "1002", swap).await.unwrap();
    f.service
        .as_origin(Origin::for_tests())
        .swap_participants(
            &f.runs[0][0],
            &["1002".to_owned()],
            &["1004".to_owned()],
            true,
            &Guild,
        )
        .await
        .unwrap();
    let head = f.service.store().history_head().await.unwrap();
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &request.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
    ));
    assert_eq!(f.service.store().history_head().await.unwrap(), head);
    // Nor may a non-participant ask to leave in the first place.
    assert!(matches!(
        submit(
            &mut f.service,
            "1003",
            RequestSpec::Leave(Subject::Run(f.runs[0][0].clone()))
        )
        .await,
        Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
    ));
}

#[tokio::test]
async fn pending_requests_are_capped() {
    let mut f = fixture().await;
    for run in [&f.runs[0][0], &f.runs[0][2], &f.runs[1][0]] {
        submit(&mut f.service, "1003", join(run)).await.unwrap();
    }
    assert!(matches!(
        submit(&mut f.service, "1003", join(&f.runs[1][2])).await,
        Err(RequestError::Limited(RequestLimit::Pending {
            count: 3,
            max: 3
        }))
    ));
    // Another member is not affected.
    submit(&mut f.service, "1004", join(&f.runs[1][2]))
        .await
        .unwrap();
}

#[tokio::test]
async fn withdrawn_and_rejected_requests_cannot_be_approved() {
    let mut f = fixture().await;
    let withdrawn = submit(&mut f.service, "1003", join(&f.runs[0][0]))
        .await
        .unwrap();
    // Only the requester withdraws.
    assert!(matches!(
        f.service.withdraw_request("1004", &withdrawn.id, 1).await,
        Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
    ));
    f.service
        .withdraw_request("1003", &withdrawn.id, 1)
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &withdrawn.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Draft(DraftError::Stale {
            status: DraftStatus::Withdrawn,
            ..
        }))
    ));
    let rejected = submit(&mut f.service, "1003", join(&f.runs[0][2]))
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .reject_request(&admin(), &rejected.id, 1, "  ")
            .await,
        Err(RequestError::Refused(RequestRefusal::ReasonRequired))
    ));
    let head = f.service.store().history_head().await.unwrap();
    let decided = f
        .service
        .reject_request(&admin(), &rejected.id, 1, "full already")
        .await
        .unwrap();
    assert_eq!(decided.request.status, DraftStatus::Rejected);
    assert_eq!(
        decided.request.close_reason.as_deref(),
        Some("full already")
    );
    assert_eq!(decided.requester_notice.listed, ["1003"]);
    assert!(matches!(
        &decided.requester_notice.change,
        NoticeChange::RequestDecided {
            decision: RequestDecision::Rejected,
            reason: Some(reason),
            ..
        } if reason == "full already"
    ));
    assert_eq!(
        f.service.store().history_head().await.unwrap(),
        head,
        "a rejection writes no schedule record"
    );
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &rejected.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Draft(DraftError::Stale {
            status: DraftStatus::Rejected,
            ..
        }))
    ));
    // Withdrawing a decided request is refused too.
    assert!(matches!(
        f.service.withdraw_request("1003", &rejected.id, 1).await,
        Err(RequestError::Draft(DraftError::Stale { .. }))
    ));
}

#[tokio::test]
async fn approval_needs_an_admin_and_the_reviewed_version() {
    let mut f = fixture().await;
    let request = submit(&mut f.service, "1003", join(&f.runs[0][0]))
        .await
        .unwrap();
    for actor in [Actor::member("1001"), Actor::system("delivery")] {
        assert!(matches!(
            f.service
                .approve_request(&actor, &request.id, 1, None, &policy(), &Guild, &NoFreezes)
                .await,
            Err(RequestError::Refused(RequestRefusal::NotAdmin))
        ));
        assert!(matches!(
            f.service.reject_request(&actor, &request.id, 1, "no").await,
            Err(RequestError::Refused(RequestRefusal::NotAdmin))
        ));
    }
    // The admin edits the request (joining the next week instead): v2.
    let edited = f
        .service
        .edit_request(
            &admin(),
            &request.id,
            1,
            vec![kanade::domain::drafts::DraftOp::SwapParticipants {
                run: kanade::domain::drafts::Target::Existing(f.runs[0][2].clone()),
                remove: Vec::new(),
                add: vec!["1003".into()],
                via_portal: true,
            }],
            &policy(),
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(edited.draft.version, 2);
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &request.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Draft(DraftError::Stale { version: 2, .. }))
    ));
    f.service
        .approve_request(
            &admin(),
            &request.id,
            2,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn frozen_members_cannot_submit_and_submits_are_idempotent() {
    let mut f = fixture().await;
    assert!(matches!(
        f.service
            .submit_request(
                "1003",
                "please",
                join(&f.runs[0][0]),
                None,
                &policy(),
                &Guild,
                &Frozen("1003"),
            )
            .await,
        Err(RequestError::Refused(RequestRefusal::Frozen))
    ));
    let first = once_as(&mut f.service, join(&f.runs[0][0])).await.unwrap();
    let again = once_as(&mut f.service, join(&f.runs[0][0])).await.unwrap();
    assert_eq!(first.id, again.id, "the exact retry returns the request");
    assert!(matches!(
        once_as(&mut f.service, join(&f.runs[0][2])).await,
        Err(RequestError::Draft(DraftError::RequestMismatch { draft_id })) if draft_id == first.id
    ));
    assert_eq!(
        f.service
            .store()
            .list_drafts(Some(DraftStatus::Submitted))
            .await
            .unwrap()
            .len(),
        1
    );
}

fn retime(fixed: &str) -> RequestSpec {
    RequestSpec::ChangeFixed {
        fixed_id: fixed.to_owned(),
        edit: FixedEdit {
            time: Some(NaiveTime::from_hms_opt(22, 0, 0).unwrap()),
            ..FixedEdit::default()
        },
    }
}

#[tokio::test]
async fn change_requests_need_choices_and_report_stale_ones() {
    let mut f = fixture().await;
    // Only day, time, party or channel.
    assert!(matches!(
        submit(
            &mut f.service,
            "1001",
            RequestSpec::ChangeFixed {
                fixed_id: f.fixed[0].clone(),
                edit: FixedEdit {
                    note: Some("x".into()),
                    ..FixedEdit::default()
                },
            }
        )
        .await,
        Err(RequestError::Refused(RequestRefusal::FieldNotAllowed))
    ));
    let request = submit(&mut f.service, "1001", retime(&f.fixed[0]))
        .await
        .unwrap();
    assert!(
        request.scope.expires_week().is_none(),
        "a weekly change does not expire by week"
    );
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &request.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Refused(RequestRefusal::ChoicesRequired))
    ));
    let keep = |runs: &[&String]| {
        FixedEditChoices::PerRun(
            runs.iter()
                .map(|run| ((*run).clone(), AmendedRunChoice::KeepForThisWeek))
                .collect::<BTreeMap<_, _>>(),
        )
    };
    // Another amendment lands upstream: the reviewed choices are stale.
    let stale = submit(&mut f.service, "1002", retime(&f.fixed[0]))
        .await
        .unwrap();
    let moved = snapshot(&f.service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == f.runs[0][2])
        .unwrap()
        .datetime
        + chrono::TimeDelta::days(1);
    f.service
        .as_origin(Origin::for_tests())
        .amend_run(&f.runs[0][2], moved, &policy())
        .await
        .unwrap();
    let result = f
        .service
        .approve_request(
            &admin(),
            &stale.id,
            1,
            Some(keep(&[&f.runs[0][1], &f.runs[0][2]])),
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await;
    assert!(
        matches!(
            &result,
            Err(RequestError::Draft(DraftError::Conflicts(conflicts)))
                if conflicts.iter().any(|conflict| matches!(
                    conflict,
                    kanade::domain::drafts::MergeConflict::ChoicesStale { .. }
                ))
        ),
        "{result:?}"
    );
    // Choices only apply to change requests.
    let joined = submit(&mut f.service, "1003", join(&f.runs[1][0]))
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &joined.id,
                1,
                Some(FixedEditChoices::UpdateAll),
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Refused(RequestRefusal::ChoicesNotApplicable))
    ));
}

#[tokio::test]
async fn a_change_request_merges_with_per_run_choices() {
    let mut f = fixture().await;
    let request = submit(&mut f.service, "1001", retime(&f.fixed[0]))
        .await
        .unwrap();
    let choices = FixedEditChoices::PerRun(BTreeMap::from([(
        f.runs[0][1].clone(),
        AmendedRunChoice::KeepForThisWeek,
    )]));
    let before = snapshot(&f.service)
        .await
        .runs
        .into_iter()
        .find(|run| run.id == f.runs[0][1])
        .unwrap()
        .datetime;
    f.service
        .approve_request(
            &admin(),
            &request.id,
            1,
            Some(choices),
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    let timing = state
        .fixed_runs
        .iter()
        .find(|row| row.id == f.fixed[0])
        .unwrap();
    assert_eq!(timing.time, NaiveTime::from_hms_opt(22, 0, 0).unwrap());
    let kept = state
        .runs
        .iter()
        .find(|run| run.id == f.runs[0][1])
        .unwrap();
    assert_eq!(kept.datetime, before, "the amended run kept its slot");
}

#[tokio::test]
async fn run_requests_expire_after_the_reset_and_weekly_ones_do_not() {
    let mut f = fixture().await;
    let weekly = submit(
        &mut f.service,
        "1003",
        RequestSpec::Join(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    assert!(weekly.scope.expires_week().is_none());
    let this_week = submit(&mut f.service, "1003", join(&f.runs[0][0]))
        .await
        .unwrap();
    assert!(this_week.scope.expires_week().is_some());
    f.service.clock().set(kl(3, 1, 0));
    let expired = f.service.expire_due_drafts(&policy()).await.unwrap();
    assert_eq!(expired.ids, std::slice::from_ref(&this_week.id));
    assert_eq!(expired.notices.len(), 1);
    let notice = &expired.notices[0];
    assert_eq!(notice.listed, ["1003"]);
    assert!(matches!(
        &notice.change,
        NoticeChange::RequestDecided { decision: RequestDecision::Expired, request, .. }
            if *request == this_week.id
    ));
    // Written by the expiry itself, keyed by the request's close; a second
    // pass finds nothing and enqueues nothing.
    let closes = async |f: &Fixture| {
        kanade::domain::notify::NoticeOutbox::outbox_notices(f.service.store())
            .await
            .unwrap()
            .into_iter()
            .filter(|row| row.source.starts_with("draft:"))
            .collect::<Vec<_>>()
    };
    let outbox = closes(&f).await;
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        outbox[0].source,
        kanade::domain::notify::draft_source(&this_week.id)
    );
    assert_eq!(&outbox[0].notice, notice);
    assert!(
        f.service
            .expire_due_drafts(&policy())
            .await
            .unwrap()
            .ids
            .is_empty()
    );
    assert_eq!(closes(&f).await.len(), 1);
    let still = f
        .service
        .store()
        .load_draft(&weekly.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(still.draft.status, DraftStatus::Submitted);
    // An expired request cannot be approved.
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &this_week.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Draft(DraftError::Expired))
    ));
}

#[tokio::test]
async fn request_and_admin_methods_refuse_each_others_drafts() {
    let mut f = fixture().await;
    let draft = f
        .service
        .create_draft("root", "admin draft", None)
        .await
        .unwrap();
    assert!(matches!(
        f.service.withdraw_request("1003", &draft.id, 1).await,
        Err(RequestError::AdminDraft)
    ));
    assert!(matches!(
        f.service
            .approve_request(&admin(), &draft.id, 1, None, &policy(), &Guild, &NoFreezes)
            .await,
        Err(RequestError::AdminDraft)
    ));
    let request = submit(&mut f.service, "1003", join(&f.runs[0][0]))
        .await
        .unwrap();
    assert!(matches!(
        f.service
            .merge_draft("root", &request.id, 1, &policy(), &Guild, None)
            .await,
        Err(DraftError::RequestDraft)
    ));
}

#[tokio::test]
async fn two_joins_on_one_timing_both_merge() {
    let mut f = fixture().await;
    let timing = RequestSpec::Join(Subject::Fixed(f.fixed[1].clone()));
    let first = submit(&mut f.service, "1003", timing.clone())
        .await
        .unwrap();
    let second = submit(&mut f.service, "1004", timing).await.unwrap();
    f.service
        .approve_request(&admin(), &first.id, 1, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
    let result = f
        .service
        .approve_request(&admin(), &second.id, 1, None, &policy(), &Guild, &NoFreezes)
        .await;
    assert!(result.is_ok(), "{result:?}");
    let state = snapshot(&f.service).await;
    let party = &state
        .fixed_runs
        .iter()
        .find(|row| row.id == f.fixed[1])
        .unwrap()
        .participants;
    for user in ["1001", "1002", "1003", "1004"] {
        assert!(party.contains(&user.to_owned()), "{party:?}");
    }
}

fn party_of(state: &ScheduleSnapshot, fixed: &str) -> Vec<String> {
    let mut party = state
        .fixed_runs
        .iter()
        .find(|row| row.id == fixed)
        .unwrap()
        .participants
        .clone();
    party.sort();
    party
}

#[tokio::test]
async fn a_leave_after_an_upstream_join_merges() {
    let mut f = fixture().await;
    let timing = Subject::Fixed(f.fixed[1].clone());
    let leave = submit(&mut f.service, "1002", RequestSpec::Leave(timing.clone()))
        .await
        .unwrap();
    let join = submit(&mut f.service, "1003", RequestSpec::Join(timing))
        .await
        .unwrap();
    f.service
        .approve_request(&admin(), &join.id, 1, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
    f.service
        .approve_request(&admin(), &leave.id, 1, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    assert_eq!(party_of(&state, &f.fixed[1]), ["1001", "1003"]);
    // The pushed runs follow the timing.
    for run in &f.runs[1] {
        let row = state.runs.iter().find(|row| row.id == *run).unwrap();
        assert!(!row.participants.contains(&"1002".to_owned()), "{row:?}");
    }
}

#[tokio::test]
async fn a_swap_after_an_upstream_change_merges() {
    let mut f = fixture().await;
    let swap = submit(
        &mut f.service,
        "1002",
        RequestSpec::Swap {
            subject: Subject::Fixed(f.fixed[1].clone()),
            with: "1004".into(),
        },
    )
    .await
    .unwrap();
    // An administrator adds 1003 to the party directly meanwhile.
    f.service
        .as_origin(Origin::for_tests())
        .update_fixed(
            &f.fixed[1],
            FixedEdit {
                participants: Some(vec!["1001".into(), "1002".into(), "1003".into()]),
                ..FixedEdit::default()
            },
            &Guild,
            &policy(),
        )
        .await
        .unwrap();
    f.service
        .approve_request(&admin(), &swap.id, 1, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
    assert_eq!(
        party_of(&snapshot(&f.service).await, &f.fixed[1]),
        ["1001", "1003", "1004"]
    );
}

#[tokio::test]
async fn joining_a_party_you_are_on_is_refused() {
    let mut f = fixture().await;
    for spec in [
        RequestSpec::Join(Subject::Fixed(f.fixed[0].clone())),
        RequestSpec::Join(Subject::Run(f.runs[0][0].clone())),
    ] {
        assert!(matches!(
            submit(&mut f.service, "1001", spec).await,
            Err(RequestError::Refused(RequestRefusal::AlreadyInParty))
        ));
    }
    assert!(matches!(
        submit(
            &mut f.service,
            "1002",
            RequestSpec::Swap {
                subject: Subject::Fixed(f.fixed[0].clone()),
                with: "1001".into(),
            }
        )
        .await,
        Err(RequestError::Refused(RequestRefusal::AlreadyInParty))
    ));
}

#[tokio::test]
async fn preview_request_analyses_and_writes_nothing() {
    let mut f = fixture().await;
    let request = submit(
        &mut f.service,
        "1002",
        RequestSpec::Leave(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    let head = f.service.store().history_head().await.unwrap();
    assert!(matches!(
        f.service
            .preview_request(
                &Actor::member("1001"),
                &request.id,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::Refused(RequestRefusal::NotAdmin))
    ));
    let preview = f
        .service
        .preview_request(
            &admin(),
            &request.id,
            None,
            &policy(),
            &Guild,
            &Frozen("1002"),
        )
        .await
        .unwrap();
    assert!(
        preview.analysis.is_clean(),
        "{:?}",
        preview.analysis.conflicts
    );
    assert!(preview.requester_authorised);
    assert!(preview.requester_frozen);
    assert_eq!(preview.version, 1);
    assert_eq!(preview.status, DraftStatus::Submitted);
    assert_eq!(f.service.store().history_head().await.unwrap(), head);
    // Frozen requesters are still approvable, and badged.
    let approved = f
        .service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &Frozen("1002"),
        )
        .await
        .unwrap();
    assert!(approved.requester_frozen);
    // Public summaries never carry the member's title.
    for notice in &approved.merge.notices {
        match &notice.change {
            NoticeChange::Merged { title, .. } => {
                assert_eq!(
                    title,
                    &format!("member request: leave fixed:{}", f.fixed[1])
                );
            }
            other => panic!("{other:?}"),
        }
    }
}

#[tokio::test]
async fn rejection_reasons_are_capped() {
    let mut f = fixture().await;
    let request = submit(&mut f.service, "1003", join(&f.runs[0][0]))
        .await
        .unwrap();
    for reason in ["x".repeat(501), "no\u{7}".to_owned(), "a\nb".to_owned()] {
        assert!(matches!(
            f.service
                .reject_request(&admin(), &request.id, 1, &reason)
                .await,
            Err(RequestError::Refused(RequestRefusal::ReasonInvalid))
        ));
    }
    let decided = f
        .service
        .reject_request(
            &admin(),
            &request.id,
            1,
            &format!("  {}  ", "y".repeat(500)),
        )
        .await
        .unwrap();
    assert_eq!(decided.request.close_reason, Some("y".repeat(500)));
}

#[tokio::test]
async fn an_exact_submit_retry_replays_before_other_checks() {
    let mut f = fixture().await;
    let first = once_as(&mut f.service, join(&f.runs[0][0])).await.unwrap();
    let again = f
        .service
        .submit_request(
            "1003",
            "please",
            join(&f.runs[0][0]),
            Some("sub-1".into()),
            &policy(),
            &Guild,
            &Frozen("1003"),
        )
        .await
        .unwrap();
    assert_eq!(again.id, first.id, "frozen since, the retry still replays");
}

#[tokio::test]
async fn approved_choices_are_recorded_in_the_merged_event() {
    let mut f = fixture().await;
    let request = submit(&mut f.service, "1001", retime(&f.fixed[0]))
        .await
        .unwrap();
    let choices = FixedEditChoices::PerRun(BTreeMap::from([(
        f.runs[0][1].clone(),
        AmendedRunChoice::KeepForThisWeek,
    )]));
    let approved = f
        .service
        .approve_request(
            &admin(),
            &request.id,
            1,
            Some(choices),
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let events = f.service.store().draft_events(&request.id).await.unwrap();
    let merged = events.last().unwrap();
    assert_eq!(merged.kind, kanade::domain::drafts::DraftEventKind::Merged);
    assert_eq!(
        merged.detail,
        Some(format!(
            "{} choices={}:keep_for_this_week",
            approved.merge.seq, f.runs[0][1]
        ))
    );
}

/// What the wrapper does around writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Twist {
    /// Before the first merge commit, remove the requester from the timing.
    RemoveRequester,
    /// Every load after a successful write fails.
    FailLoadsAfterWrite,
}

struct Twisted<'a> {
    inner: &'a MemoryScheduleStore,
    twist: Twist,
    fixed: String,
    requester: String,
    merges: std::sync::atomic::AtomicUsize,
    written: std::sync::atomic::AtomicBool,
}

impl Twisted<'_> {
    fn broken(&self) -> Result<(), kanade::domain::scheduler::StoreError> {
        if self.twist == Twist::FailLoadsAfterWrite
            && self.written.load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(kanade::domain::scheduler::StoreError::Backend(
                "read after write".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn wrote(&self) {
        self.written
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

type StoreResult<T> = Result<T, kanade::domain::scheduler::StoreError>;

impl ScheduleStore for Twisted<'_> {
    async fn load(&self, scope: &Scope) -> StoreResult<ScheduleSnapshot> {
        self.broken()?;
        self.inner.load(scope).await
    }

    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> StoreResult<Option<kanade::domain::scheduler::RecordedRequest>> {
        self.inner.recorded_request(actor, request_id).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: kanade::domain::schedule::ChangeSet,
        meta: kanade::domain::history::ChangeMeta,
    ) -> StoreResult<Option<kanade::domain::scheduler::Committed>> {
        self.inner.commit(expected_revision, changes, meta).await
    }
}

impl DraftStore for Twisted<'_> {
    async fn snapshot_with_head(
        &self,
    ) -> StoreResult<(ScheduleSnapshot, kanade::domain::history::ChangeRef)> {
        self.broken()?;
        self.inner.snapshot_with_head().await
    }

    async fn records_after(
        &self,
        base: &kanade::domain::history::ChangeRef,
    ) -> StoreResult<Vec<kanade::domain::history::ChangeRecord>> {
        self.inner.records_after(base).await
    }

    async fn create_draft(
        &self,
        new: kanade::domain::drafts::NewDraft,
    ) -> StoreResult<kanade::domain::drafts::DraftCreated> {
        self.inner.create_draft(new).await
    }

    async fn load_draft(
        &self,
        id: &str,
    ) -> StoreResult<Option<kanade::domain::drafts::LoadedDraft>> {
        self.broken()?;
        self.inner.load_draft(id).await
    }

    async fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> StoreResult<Option<(String, kanade::domain::drafts::StoredDraft)>> {
        self.inner.recorded_draft_request(author, request_id).await
    }

    async fn list_drafts(
        &self,
        status: Option<DraftStatus>,
    ) -> StoreResult<Vec<kanade::domain::drafts::StoredDraft>> {
        self.inner.list_drafts(status).await
    }

    async fn draft_events(&self, id: &str) -> StoreResult<Vec<kanade::domain::drafts::DraftEvent>> {
        self.inner.draft_events(id).await
    }

    async fn update_draft(
        &self,
        update: kanade::domain::drafts::DraftUpdate,
    ) -> StoreResult<kanade::domain::drafts::DraftWrite> {
        let written = self.inner.update_draft(update).await?;
        self.wrote();
        Ok(written)
    }

    async fn commit_merge(
        &self,
        expected_revision: u64,
        changes: kanade::domain::schedule::ChangeSet,
        meta: kanade::domain::history::ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> StoreResult<kanade::domain::drafts::MergeCommit> {
        let first = self
            .merges
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0;
        if first && self.twist == Twist::RemoveRequester {
            let state = self.inner.load(&Scope::All).await?;
            let mut row = state
                .fixed_runs
                .iter()
                .find(|row| row.id == self.fixed)
                .unwrap()
                .clone();
            row.participants.retain(|user| *user != self.requester);
            let upstream = kanade::domain::history::ChangeMeta {
                origin: Origin::new(Actor::admin("upstream"), Surface::AdminPortal),
                at: state
                    .runs
                    .first()
                    .map(|run| run.datetime)
                    .unwrap_or_default(),
                notices: Vec::new(),
                refs: Vec::new(),
                request_digest: None,
                expect: Default::default(),
                outbox: Vec::new(),
            };
            self.inner
                .commit(
                    state.revision,
                    kanade::domain::schedule::ChangeSet {
                        changes: vec![kanade::domain::schedule::Change::PutFixedRun(row)],
                    },
                    upstream,
                )
                .await?;
        }
        let merged = self
            .inner
            .commit_merge(
                expected_revision,
                changes,
                meta,
                draft_id,
                expected_version,
                note,
            )
            .await?;
        self.wrote();
        Ok(merged)
    }

    async fn expire_drafts(
        &self,
        week: DateTime<chrono::Utc>,
        at: DateTime<chrono::Utc>,
        actor: &Actor,
        notices: Vec<(String, kanade::domain::schedule::Notice)>,
    ) -> StoreResult<Vec<String>> {
        self.inner.expire_drafts(week, at, actor, notices).await
    }
}

fn twisted<'a>(f: &'a Fixture, twist: Twist, requester: &str) -> Twisted<'a> {
    Twisted {
        inner: f.service.store(),
        twist,
        fixed: f.fixed[1].clone(),
        requester: requester.to_owned(),
        merges: std::sync::atomic::AtomicUsize::new(0),
        written: std::sync::atomic::AtomicBool::new(false),
    }
}

#[tokio::test]
async fn a_requester_removed_before_the_commit_is_refused() {
    let mut f = fixture().await;
    let request = submit(
        &mut f.service,
        "1002",
        RequestSpec::Swap {
            subject: Subject::Fixed(f.fixed[1].clone()),
            with: "1003".into(),
        },
    )
    .await
    .unwrap();
    let store = twisted(&f, Twist::RemoveRequester, "1002");
    let mut service = SchedulerService::new(&store, RandomIds, TestClock::new(kl(27, 1, 0)));
    let result = service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await;
    assert!(
        matches!(
            result,
            Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
        ),
        "{result:?}"
    );
    assert_eq!(
        store.merges.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the raced commit conflicted and the retry never committed"
    );
    let stored = f
        .service
        .store()
        .load_draft(&request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.draft.status, DraftStatus::Submitted);
}

#[tokio::test]
async fn committed_decisions_never_fail_on_later_reads() {
    let f = fixture().await;
    let mut service =
        SchedulerService::new(f.service.store(), RandomIds, TestClock::new(kl(27, 1, 0)));
    let approve = service
        .submit_request(
            "1003",
            "please",
            join(&f.runs[0][0]),
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let reject = service
        .submit_request(
            "1003",
            "please",
            join(&f.runs[0][2]),
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    for (id, approving) in [(&approve.id, true), (&reject.id, false)] {
        let store = twisted(&f, Twist::FailLoadsAfterWrite, "1003");
        let mut service = SchedulerService::new(&store, RandomIds, TestClock::new(kl(27, 1, 0)));
        if approving {
            let approved = service
                .approve_request(&admin(), id, 1, None, &policy(), &Guild, &NoFreezes)
                .await
                .unwrap();
            assert_eq!(approved.requester_notice.listed, ["1003"]);
        } else {
            let rejected = service
                .reject_request(&admin(), id, 1, "no room")
                .await
                .unwrap();
            assert_eq!(rejected.requester_notice.listed, ["1003"]);
        }
        assert!(store.written.load(std::sync::atomic::Ordering::SeqCst));
    }
}

fn run_row(state: &ScheduleSnapshot, run: &str) -> kanade::domain::schedule::Run {
    state.runs.iter().find(|row| row.id == run).unwrap().clone()
}

fn answered(state: &ScheduleSnapshot, run: &str) -> Vec<String> {
    let mut users: Vec<String> = state
        .rsvps
        .iter()
        .filter(|row| row.run_id == run)
        .map(|row| row.user_id.clone())
        .collect();
    users.sort();
    users
}

async fn substitute(f: &mut Fixture, run: &str, out: &str, into: &str) {
    f.service
        .as_origin(Origin::for_tests())
        .swap_participants(run, &[out.to_owned()], &[into.to_owned()], true, &Guild)
        .await
        .unwrap();
}

async fn answer(f: &mut Fixture, run: &str, user: &str) {
    f.service
        .as_origin(Origin::for_tests())
        .set_rsvp(
            run,
            user,
            kanade::domain::schedule::RsvpState::Yes,
            kanade::domain::schedule::RsvpSource::Chat,
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn a_timing_join_merges_after_an_upstream_one_off_substitution() {
    let mut f = fixture().await;
    let request = submit(
        &mut f.service,
        "1003",
        RequestSpec::Join(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    let run = f.runs[1][0].clone();
    substitute(&mut f, &run, "1002", "1004").await;
    f.service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    let mut party = run_row(&state, &run).participants;
    party.sort();
    assert_eq!(party, ["1001", "1003", "1004"], "the substitute stays");
    assert_eq!(party_of(&state, &f.fixed[1]), ["1001", "1002", "1003"]);
}

#[tokio::test]
async fn a_substitution_made_before_submit_survives_approval() {
    let mut f = fixture().await;
    let run = f.runs[1][0].clone();
    substitute(&mut f, &run, "1002", "1004").await;
    answer(&mut f, &run, "1004").await;
    let request = submit(
        &mut f.service,
        "1003",
        RequestSpec::Join(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    f.service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    let mut party = run_row(&state, &run).participants;
    party.sort();
    assert_eq!(party, ["1001", "1003", "1004"]);
    assert_eq!(
        answered(&state, &run),
        ["1004"],
        "the substitute's answer stays"
    );
}

#[tokio::test]
async fn a_timing_leave_drops_only_the_leavers_answers() {
    let mut f = fixture().await;
    let runs = [f.runs[1][0].clone(), f.runs[1][2].clone()];
    for run in &runs {
        answer(&mut f, run, "1001").await;
        answer(&mut f, run, "1002").await;
    }
    // A one-off substitute on the middle run must survive the leave (the
    // old whole-party push would have put 1001 back).
    let middle = f.runs[1][1].clone();
    substitute(&mut f, &middle, "1001", "1004").await;
    let request = submit(
        &mut f.service,
        "1002",
        RequestSpec::Leave(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    f.service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    for run in &runs {
        assert_eq!(answered(&state, run), ["1001"], "{run}");
        assert!(
            !run_row(&state, run)
                .participants
                .contains(&"1002".to_owned())
        );
    }
    assert_eq!(run_row(&state, &middle).participants, ["1004"]);
}

#[tokio::test]
async fn a_request_already_in_effect_is_flagged_not_closed() {
    let mut f = fixture().await;
    let request = submit(
        &mut f.service,
        "1003",
        RequestSpec::Join(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    // An administrator adds 1003 directly meanwhile.
    f.service
        .as_origin(Origin::for_tests())
        .update_fixed(
            &f.fixed[1],
            FixedEdit {
                participants: Some(vec!["1001".into(), "1002".into(), "1003".into()]),
                ..FixedEdit::default()
            },
            &Guild,
            &policy(),
        )
        .await
        .unwrap();
    let preview = f
        .service
        .preview_request(&admin(), &request.id, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
    assert!(preview.no_effect, "{:?}", preview.analysis.conflicts);
    let head = f.service.store().history_head().await.unwrap();
    assert!(matches!(
        f.service
            .approve_request(
                &admin(),
                &request.id,
                1,
                None,
                &policy(),
                &Guild,
                &NoFreezes
            )
            .await,
        Err(RequestError::NoEffect)
    ));
    assert_eq!(f.service.store().history_head().await.unwrap(), head);
    let stored = f
        .service
        .store()
        .load_draft(&request.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.draft.status,
        DraftStatus::Submitted,
        "no automatic close"
    );
    // A fresh request is not flagged.
    let fresh = submit(
        &mut f.service,
        "1004",
        RequestSpec::Join(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    assert!(
        !f.service
            .preview_request(&admin(), &fresh.id, None, &policy(), &Guild, &NoFreezes)
            .await
            .unwrap()
            .no_effect
    );
}

#[tokio::test]
async fn a_delta_that_would_empty_a_run_skips_it_and_warns() {
    let mut f = fixture().await;
    let lonely = f.runs[1][0].clone();
    // This week only 1002 plays that run.
    f.service
        .as_origin(Origin::for_tests())
        .swap_participants(&lonely, &["1001".to_owned()], &[], true, &Guild)
        .await
        .unwrap();
    let request = submit(
        &mut f.service,
        "1002",
        RequestSpec::Leave(Subject::Fixed(f.fixed[1].clone())),
    )
    .await
    .unwrap();
    let skipped = [kanade::domain::scheduler::MergeWarning::PartyDeltaSkipped {
        run_id: lonely.clone(),
        reason: kanade::domain::scheduler::SkipReason::Emptied,
    }];
    // The preview shows the run the approval would skip.
    let preview = f
        .service
        .preview_request(&admin(), &request.id, None, &policy(), &Guild, &NoFreezes)
        .await
        .unwrap();
    assert_eq!(preview.warnings, skipped);
    let approved = f
        .service
        .approve_request(
            &admin(),
            &request.id,
            1,
            None,
            &policy(),
            &Guild,
            &NoFreezes,
        )
        .await
        .unwrap();
    assert_eq!(approved.merge.warnings, skipped);
    // The merged event keeps the skipped runs for a retry after a lost
    // response.
    let events = f.service.store().draft_events(&request.id).await.unwrap();
    assert_eq!(
        events.last().unwrap().detail,
        Some(format!("{} skipped={lonely}", approved.merge.seq))
    );
    let state = snapshot(&f.service).await;
    assert_eq!(
        run_row(&state, &lonely).participants,
        ["1002"],
        "left as it was"
    );
    assert_eq!(party_of(&state, &f.fixed[1]), ["1001"]);
}
