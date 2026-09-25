//! Extractor and chatbot proposals over the in-memory store: who may
//! approve, idempotent approval, supersede, up-front refusal, refusals at ✅
//! when the schedule moved, attendance rules and TTL expiry.

use chrono::{DateTime, FixedOffset, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::domain::attendance::AttendancePolicy;
use kanade::domain::drafts::RequestLimit;
use kanade::domain::drafts::{DraftStatus, ProposalSource, ProposalStore};
use kanade::domain::history::{Actor, ChangeHistory, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::{Directory, Member};
use kanade::domain::proposals::{Approver, ChangeKind, ProposedChange, Refusal};
use kanade::domain::requests::{NoFreezes, RequestSpec, Subject};
use kanade::domain::schedule::{
    NewFixedRun, NewRun, NoticeChange, ReminderPolicy, RsvpState, RunSource, RunStatus,
    SchedulePolicy, ScheduleSnapshot, StatusChange,
};
use kanade::domain::scheduler::{
    DraftError, ProposalError, ProposalRequest, RequestError, ScheduleStore, SchedulerService,
    Scope, Supersede,
};
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
                has_role: true,
                ..Member::default()
            })
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

fn kl(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<FixedOffset> {
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
}

fn utc(at: DateTime<FixedOffset>) -> DateTime<Utc> {
    at.with_timezone(&Utc)
}

fn policy(attendance: AttendancePolicy) -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
    .with_attendance(attendance)
}

fn member(id: &str) -> Approver {
    Approver {
        user_id: id.into(),
        has_role: true,
        is_admin: false,
    }
}

/// A standalone run (party 1001–1003, channel 222) and a weekly timing
/// owned by 1004 (party 1001, 1002) with its runs, on Thu 27 Aug 01:00.
struct Fixture {
    service: Service,
    clock: TestClock,
    policy: SchedulePolicy,
    run: String,
    timing_run: String,
}

async fn fixture(attendance: AttendancePolicy) -> Fixture {
    let clock = TestClock::new(kl(8, 27, 1, 0));
    let policy = policy(attendance);
    let mut service = SchedulerService::new(MemoryScheduleStore::new(), RandomIds, clock.clone())
        .with_attendance(attendance);
    let week = utc(kl(8, 27, 0, 0));
    let run = service
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some("222".into()),
            week_start: week,
            datetime: utc(kl(8, 31, 21, 30)),
            bosses: vec!["HFA".into()],
            participants: vec!["1001".into(), "1002".into(), "1003".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .unwrap();
    let fixed = service
        .as_origin(Origin::for_tests())
        .add_fixed_run(NewFixedRun {
            owner_id: "1004".into(),
            channel_id: Some("333".into()),
            bosses: vec!["HLimbo".into()],
            weekday: Weekday::Tue,
            time: NaiveTime::from_hms_opt(21, 0, 0).unwrap(),
            participants: vec!["1001".into(), "1002".into()],
            note: None,
        })
        .await
        .unwrap();
    service
        .as_origin(Origin::for_tests())
        .materialise_weeks(&policy)
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let timing_run = state
        .runs
        .iter()
        .find(|row| row.fixed_run_id.as_deref() == Some(fixed.as_str()) && row.week_start == week)
        .unwrap()
        .id
        .clone();
    Fixture {
        service,
        clock,
        policy,
        run,
        timing_run,
    }
}

async fn snapshot(service: &Service) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

fn cancel(run: &str, channel: &str) -> ProposedChange {
    ProposedChange {
        run_id: Some(run.into()),
        channel_id: Some(channel.into()),
        ..ProposedChange::new(ChangeKind::Cancel)
    }
}

fn answer(run: &str, user: &str, state: RsvpState) -> ProposedChange {
    ProposedChange {
        run_id: Some(run.into()),
        channel_id: Some("222".into()),
        participants: vec![user.into()],
        rsvp: Some(state),
        ..ProposedChange::new(ChangeKind::Rsvp)
    }
}

impl Fixture {
    async fn propose_from(
        &mut self,
        change: ProposedChange,
        source: ProposalSource,
        supersede: Supersede,
    ) -> Result<String, ProposalError> {
        self.service
            .propose(
                ProposalRequest {
                    change,
                    source,
                    source_id: "log-1".into(),
                    supersede,
                },
                &self.policy,
                &Guild,
            )
            .await
            .map(|proposed| proposed.proposal.id)
    }

    async fn propose(&mut self, change: ProposedChange) -> String {
        self.propose_from(change, ProposalSource::Extraction, Supersede::Older)
            .await
            .unwrap()
    }

    async fn approve(
        &mut self,
        id: &str,
        approver: &Approver,
    ) -> Result<kanade::domain::scheduler::ProposalApproved, ProposalError> {
        self.service
            .approve_proposal(id, approver, &self.policy, &Guild)
            .await
    }

    async fn status(&self, id: &str) -> DraftStatus {
        let (loaded, _) = self
            .service
            .store()
            .load_proposal(id)
            .await
            .unwrap()
            .unwrap();
        loaded.draft.status
    }

    async fn run_status(&self, id: &str) -> RunStatus {
        let state = snapshot(&self.service).await;
        state.runs.iter().find(|run| run.id == id).unwrap().status
    }
}

#[tokio::test]
async fn a_participant_approves_through_the_source_surface_with_merge_notices() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let id = f.propose(cancel(&f.run.clone(), "222")).await;
    let approved = f.approve(&id, &member("1002")).await.unwrap();
    assert_eq!(f.run_status(&f.run.clone()).await, RunStatus::Cancelled);
    assert_eq!(f.status(&id).await, DraftStatus::Merged);
    let record = f
        .service
        .store()
        .load_change(approved.merge.seq)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.actor, Actor::member("1002"));
    assert_eq!(record.origin.surface, Surface::ExtractionApproval);
    // The same outbox notices as a draft merge: one summary per channel.
    assert_eq!(approved.merge.notices.len(), 1);
    assert_eq!(approved.merge.notices[0].channel_id.as_deref(), Some("222"));
    assert!(matches!(
        &approved.merge.notices[0].change,
        NoticeChange::Merged { draft, title, .. } if draft == &id && title == "cancel proposal"
    ));

    let timing_run = f.timing_run.clone();
    let chat = f
        .propose_from(
            cancel(&timing_run, "333"),
            ProposalSource::Chat,
            Supersede::Older,
        )
        .await
        .unwrap();
    let approved = f.approve(&chat, &member("1001")).await.unwrap();
    let record = f
        .service
        .store()
        .load_change(approved.merge.seq)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.surface, Surface::ChatApproval);
}

#[tokio::test]
async fn only_participants_admins_and_the_owner_may_answer() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let id = f.propose(cancel(&run, "222")).await;
    let before = snapshot(&f.service).await;
    let refused = [
        member("1004"),
        Approver {
            has_role: false,
            ..member("1001")
        },
    ];
    for approver in &refused {
        assert_eq!(
            f.approve(&id, approver).await.unwrap_err(),
            ProposalError::Unauthorised
        );
        assert_eq!(
            f.service.reject_proposal(&id, approver).await.unwrap_err(),
            ProposalError::Unauthorised
        );
    }
    // Nothing moved: no record, no status change.
    assert_eq!(snapshot(&f.service).await, before);
    assert_eq!(f.status(&id).await, DraftStatus::Submitted);
    let admin = Approver {
        has_role: false,
        is_admin: true,
        ..member("9999")
    };
    f.approve(&id, &admin).await.unwrap();

    // 1004 owns the weekly timing, so may answer for its run.
    let timing_run = f.timing_run.clone();
    let owned = f.propose(cancel(&timing_run, "333")).await;
    f.approve(&owned, &member("1004")).await.unwrap();
    assert_eq!(f.run_status(&timing_run).await, RunStatus::Cancelled);
}

#[tokio::test]
async fn a_repeated_approval_applies_once_and_posts_nothing() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let id = f.propose(cancel(&run, "222")).await;
    let first = f.approve(&id, &member("1002")).await.unwrap();
    let after = snapshot(&f.service).await;
    assert_eq!(
        f.approve(&id, &member("1002")).await.unwrap_err(),
        ProposalError::Draft(DraftError::AlreadyApplied {
            seq: first.merge.seq,
            revision: first.merge.revision,
        })
    );
    assert!(matches!(
        f.approve(&id, &member("1001")).await.unwrap_err(),
        ProposalError::Draft(DraftError::AlreadyMerged { seq }) if seq == first.merge.seq
    ));
    assert_eq!(snapshot(&f.service).await, after);
}

#[tokio::test]
async fn a_newer_proposal_supersedes_the_live_one_for_its_target() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let older = f.propose(cancel(&run, "222")).await;
    let elsewhere = f.propose(cancel(&run, "444")).await;
    let proposed = f
        .service
        .propose(
            ProposalRequest {
                change: ProposedChange {
                    kind: ChangeKind::Otot,
                    ..cancel(&run, "222")
                },
                source: ProposalSource::Extraction,
                source_id: "log-2".into(),
                supersede: Supersede::Older,
            },
            &f.policy,
            &Guild,
        )
        .await
        .unwrap();
    assert_eq!(proposed.superseded, std::slice::from_ref(&older));
    let (loaded, _) = f
        .service
        .store()
        .load_proposal(&older)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.draft.status, DraftStatus::Discarded);
    assert_eq!(loaded.draft.close_reason.as_deref(), Some("superseded"));
    assert_eq!(loaded.draft.closed_by, Some(Actor::system("extraction")));
    // Another channel's card is left alone.
    assert_eq!(f.status(&elsewhere).await, DraftStatus::Submitted);
}

#[tokio::test]
async fn approving_retires_the_sibling_proposals() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let mut ids = Vec::new();
    for change in [cancel(&run, "222"), answer(&run, "1001", RsvpState::Yes)] {
        ids.push(
            f.propose_from(change, ProposalSource::Extraction, Supersede::Keep)
                .await
                .unwrap(),
        );
    }
    let approved = f.approve(&ids[1], &member("1001")).await.unwrap();
    assert_eq!(approved.superseded, [ids[0].clone()]);
    let (loaded, _) = f
        .service
        .store()
        .load_proposal(&ids[0])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.draft.close_reason.as_deref(), Some("superseded"));
}

#[tokio::test]
async fn an_unappliable_change_is_refused_up_front_and_writes_nothing() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let before = snapshot(&f.service).await;
    let refused = f
        .propose_from(
            ProposedChange {
                run_id: Some(run.clone()),
                ..ProposedChange::new(ChangeKind::Move)
            },
            ProposalSource::Chat,
            Supersede::Older,
        )
        .await
        .unwrap_err();
    assert_eq!(refused, ProposalError::Refused(Refusal::NoNewTime));
    assert_eq!(
        refused.to_string(),
        "no new time was agreed - use `/amend` to set one"
    );
    // Already in effect: nothing to card either.
    f.propose(cancel(&run, "222")).await;
    let only = f.service.store().list_proposals(false).await.unwrap();
    assert_eq!(only.len(), 1);
    assert_eq!(snapshot(&f.service).await, before);
    let id = only[0].draft.id.clone();
    f.approve(&id, &member("1001")).await.unwrap();
    assert_eq!(
        f.propose_from(cancel(&run, "222"), ProposalSource::Chat, Supersede::Older)
            .await
            .unwrap_err(),
        ProposalError::NoEffect
    );
}

#[tokio::test]
async fn an_answer_for_someone_swapped_off_meanwhile_is_refused_at_approval() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let id = f.propose(answer(&run, "1003", RsvpState::Yes)).await;
    f.service
        .as_origin(Origin::for_tests())
        .swap_participants(&run, &["1003".into()], &["1004".into()], false, &Guild)
        .await
        .unwrap();
    let refused = f.approve(&id, &member("1001")).await.unwrap_err();
    assert_eq!(refused, ProposalError::Refused(Refusal::AnswerForOutsider));
    assert_eq!(
        refused.to_string(),
        "that answer is for somebody who is no longer on the run"
    );
    assert_eq!(f.status(&id).await, DraftStatus::Submitted);
}

#[tokio::test]
async fn proposed_answers_recount_but_never_end_a_pin_or_move_a_started_run() {
    let mut f = fixture(AttendancePolicy::V5).await;
    let run = f.run.clone();
    f.service
        .as_origin(Origin::for_tests())
        .set_status(
            &run,
            StatusChange {
                status: RunStatus::Confirmed,
                announce: false,
                via_portal: true,
            },
            &f.policy.reminders,
        )
        .await
        .unwrap();
    let id = f.propose(answer(&run, "1001", RsvpState::No)).await;
    f.approve(&id, &member("1002")).await.unwrap();
    let state = snapshot(&f.service).await;
    let row = state.runs.iter().find(|row| row.id == run).unwrap();
    assert_eq!(row.status, RunStatus::Confirmed);
    assert!(row.status_pin.is_some(), "a proposed answer keeps the pin");
    assert!(
        state.rsvps.iter().any(|rsvp| rsvp.run_id == run
            && rsvp.user_id == "1001"
            && rsvp.state == RsvpState::No)
    );

    // Frozen once started: an answer applied after the start changes no status.
    let tonight = f
        .service
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some("222".into()),
            week_start: utc(kl(8, 27, 0, 0)),
            datetime: utc(kl(8, 27, 20, 0)),
            bosses: vec!["HLimbo".into()],
            participants: vec!["1001".into(), "1002".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .unwrap();
    let late = f
        .propose_from(
            answer(&tonight, "1001", RsvpState::No),
            ProposalSource::Chat,
            Supersede::Older,
        )
        .await
        .unwrap();
    let before = f.run_status(&tonight).await;
    f.clock.set(kl(8, 27, 20, 5));
    f.approve(&late, &member("1002")).await.unwrap();
    assert_eq!(f.run_status(&tonight).await, before);
}

#[tokio::test]
async fn a_proposed_answer_recounts_an_unpinned_run() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let id = f.propose(answer(&run, "1003", RsvpState::No)).await;
    f.approve(&id, &member("1001")).await.unwrap();
    assert_eq!(f.run_status(&run).await, RunStatus::AtRisk);
}

#[tokio::test]
async fn a_proposal_past_its_ttl_is_refused_and_closed() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let id = f.propose(cancel(&run, "222")).await;
    f.clock.set(kl(8, 28, 1, 0));
    assert_eq!(
        f.approve(&id, &member("1001")).await.unwrap_err(),
        ProposalError::Expired
    );
    assert_eq!(f.status(&id).await, DraftStatus::Expired);
    assert_eq!(f.run_status(&run).await, RunStatus::Planned);
    assert_eq!(
        f.service.expire_due_proposals().await.unwrap(),
        Vec::<String>::new()
    );
}

#[tokio::test]
async fn proposals_leave_member_request_limits_alone() {
    let mut f = fixture(AttendancePolicy::V4_COMPAT).await;
    let run = f.run.clone();
    let mut timing_runs: Vec<_> = snapshot(&f.service)
        .await
        .runs
        .into_iter()
        .filter(|row| row.fixed_run_id.is_some())
        .collect();
    timing_runs.sort_by_key(|row| row.datetime);
    let subjects: Vec<String> = std::iter::once(run.clone())
        .chain(timing_runs.into_iter().map(|row| row.id))
        .collect();
    assert_eq!(subjects.len(), 4);
    let join = async |f: &mut Fixture, subject: &str| {
        f.service
            .submit_request(
                "1004",
                "please",
                RequestSpec::Join(Subject::Run(subject.to_owned())),
                None,
                &f.policy,
                &Guild,
                &NoFreezes,
            )
            .await
    };
    for subject in &subjects[..2] {
        join(&mut f, subject).await.unwrap();
    }
    // 1004 approves a proposal (as an administrator) while two requests wait.
    let id = f.propose(cancel(&run, "222")).await;
    let admin = Approver {
        is_admin: true,
        ..member("1004")
    };
    f.approve(&id, &admin).await.unwrap();
    // The third still fits the cap of three; only the fourth is over it.
    join(&mut f, &subjects[2]).await.unwrap();
    assert!(matches!(
        join(&mut f, &subjects[3]).await,
        Err(RequestError::Limited(RequestLimit::Pending {
            count: 3,
            max: 3
        }))
    ));
}
