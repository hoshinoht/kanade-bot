//! Member requests over a temp SQLite store: submit, limits, withdraw,
//! admin edit, approve (merge) and reject, expiry and requester notices.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use chrono::{DateTime, TimeDelta, Utc};
use kanade::domain::attendance::AttendancePolicy;
use kanade::domain::drafts::{
    DraftCreated, DraftEvent, DraftEventKind, DraftOp, DraftStatus, DraftStore, DraftUpdate,
    DraftWrite, LoadedDraft, MergeCommit, MergeConflict, NewDraft, RequestLimit, StoredDraft,
    Target,
};
use kanade::domain::history::{
    Actor, BlameTarget, ChangeHistory, ChangeMeta, ChangeRecord, ChangeRef, Origin, Surface, Via,
    blame,
};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{DeliverySettings, NoticeOutbox, draft_source, plan_notice};
use kanade::domain::requests::{MemberGate, NoFreezes, RequestRefusal, RequestSpec, Subject};
use kanade::domain::schedule::{
    AmendedRunChoice, Change, ChangeSet, FixedEdit, FixedEditChoices, Notice, NoticeChange,
    RequestDecision, RsvpSource, RsvpState, Run, ScheduleSnapshot,
};
use kanade::domain::scheduler::{
    Approved, Committed, DraftError, MergeWarning, RecordedRequest, RequestError, RequestPreview,
    RequestResult, ScheduleStore, SchedulerService, Scope, SkipReason, StoreError,
};
use kanade::infrastructure::store::SqliteStore;

use crate::common::TestClock;
use crate::support::{
    AnyChannel, Guild, TwoTimings as Fixture, by_tests, hm, kl, owned, policy, snapshot,
    two_timings as fixture,
};

type Svc<S> = SchedulerService<S, RandomIds, TestClock>;
type Service = Svc<SqliteStore>;

struct Frozen(&'static str);

impl MemberGate for Frozen {
    fn is_frozen(&self, user_id: &str) -> bool {
        user_id == self.0
    }
}

fn admin() -> Actor {
    Actor::admin("root")
}

async fn submit(
    service: &mut Service,
    member: &str,
    spec: RequestSpec,
) -> RequestResult<StoredDraft> {
    service
        .submit_request(member, "please", spec, None, &policy(), &Guild, &NoFreezes)
        .await
}

/// A submit that must succeed.
async fn filed(service: &mut Service, member: &str, spec: RequestSpec) -> StoredDraft {
    submit(service, member, spec).await.unwrap()
}

/// Submit as 1003 with the request id `sub-1`.
async fn once_as(
    service: &mut Service,
    spec: RequestSpec,
    gate: &(dyn MemberGate + Sync),
) -> RequestResult<StoredDraft> {
    let (policy, key) = (policy(), Some("sub-1".into()));
    service
        .submit_request("1003", "please", spec, key, &policy, &Guild, gate)
        .await
}

/// Approve version `version` of the request with `choices` as `actor`.
async fn approve_as<S: ScheduleStore + DraftStore>(
    service: &mut Svc<S>,
    actor: &Actor,
    id: &str,
    version: u64,
    choices: Option<FixedEditChoices>,
    gate: &(dyn MemberGate + Sync),
) -> RequestResult<Approved> {
    let policy = policy();
    service
        .approve_request(actor, id, version, choices, &policy, &Guild, gate)
        .await
}

/// Approve as the admin, without choices or freezes.
async fn approve<S: ScheduleStore + DraftStore>(
    service: &mut Svc<S>,
    id: &str,
    version: u64,
) -> RequestResult<Approved> {
    approve_as(service, &admin(), id, version, None, &NoFreezes).await
}

/// Approve version 1 as the admin with `choices`, without freezes.
async fn approve_with(
    service: &mut Service,
    id: &str,
    choices: Option<FixedEditChoices>,
) -> RequestResult<Approved> {
    approve_as(service, &admin(), id, 1, choices, &NoFreezes).await
}

async fn preview(
    service: &Service,
    actor: &Actor,
    id: &str,
    gate: &(dyn MemberGate + Sync),
) -> RequestResult<RequestPreview> {
    service
        .preview_request(actor, id, None, &policy(), &Guild, gate)
        .await
}

async fn stored(service: &Service, id: &str) -> LoadedDraft {
    service.store().load_draft(id).await.unwrap().unwrap()
}

fn join(run: &str) -> RequestSpec {
    RequestSpec::Join(Subject::Run(run.to_owned()))
}

fn join_timing(fixed: &str) -> RequestSpec {
    RequestSpec::Join(Subject::Fixed(fixed.to_owned()))
}

fn leave_timing(fixed: &str) -> RequestSpec {
    RequestSpec::Leave(Subject::Fixed(fixed.to_owned()))
}

fn swap_timing(fixed: &str, with: &str) -> RequestSpec {
    RequestSpec::Swap {
        subject: Subject::Fixed(fixed.to_owned()),
        with: with.into(),
    }
}

fn keep_for_this_week(runs: &[&String]) -> FixedEditChoices {
    FixedEditChoices::PerRun(
        runs.iter()
            .map(|run| ((*run).clone(), AmendedRunChoice::KeepForThisWeek))
            .collect::<BTreeMap<_, _>>(),
    )
}

fn run_row(state: &ScheduleSnapshot, run: &str) -> Run {
    state.runs.iter().find(|row| row.id == run).unwrap().clone()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn party_of(state: &ScheduleSnapshot, fixed: &str) -> Vec<String> {
    let timing = state.fixed_runs.iter().find(|row| row.id == fixed).unwrap();
    sorted(timing.participants.clone())
}

fn answered(state: &ScheduleSnapshot, run: &str) -> Vec<String> {
    let rows = state.rsvps.iter().filter(|row| row.run_id == run);
    sorted(rows.map(|row| row.user_id.clone()).collect())
}

/// An administrator sets a timing's party directly.
async fn set_party(f: &mut Fixture, fixed: &str, party: &[&str]) {
    let edit = FixedEdit {
        participants: Some(owned(party)),
        ..FixedEdit::default()
    };
    by_tests(&mut f.service)
        .update_fixed(fixed, edit, &Guild, &policy())
        .await
        .unwrap();
}

async fn substitute(f: &mut Fixture, run: &str, out: &[&str], into: &[&str]) {
    by_tests(&mut f.service)
        .swap_participants(run, &owned(out), &owned(into), true, &Guild)
        .await
        .unwrap();
}

async fn answer(f: &mut Fixture, run: &str, user: &str) {
    by_tests(&mut f.service)
        .set_rsvp(run, user, RsvpState::Yes, RsvpSource::Chat)
        .await
        .unwrap();
}

#[tokio::test]
async fn an_approved_request_merges_as_the_admin_and_tells_only_the_requester() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1003", join(&f.runs[0][0])).await;
    assert_eq!(request.status, DraftStatus::Submitted);
    assert_eq!(request.request_type.as_deref(), Some("join"));
    let base = request.base.clone();
    let approved = approve(&mut f.service, &request.id, 1).await.unwrap();
    let run = run_row(&snapshot(&f.service).await, &f.runs[0][0]);
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
    let stored = stored(&f.service, &request.id).await;
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
    let settings = DeliverySettings {
        post_channel_id: None,
        quiet_mode: false,
        attendance: AttendancePolicy::V4_COMPAT,
    };
    let intent = plan_notice(notice, &Guild, &AnyChannel, settings).unwrap();
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
    let request = filed(&mut f.service, "1002", swap).await;
    let run = f.runs[0][0].clone();
    substitute(&mut f, &run, &["1002"], &["1004"]).await;
    let head = f.service.store().history_head().await.unwrap();
    assert!(matches!(
        approve(&mut f.service, &request.id, 1).await,
        Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
    ));
    assert_eq!(f.service.store().history_head().await.unwrap(), head);
    // Nor may a non-participant ask to leave in the first place.
    let leave = RequestSpec::Leave(Subject::Run(run));
    assert!(matches!(
        submit(&mut f.service, "1003", leave).await,
        Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
    ));
}

#[tokio::test]
async fn pending_requests_are_capped() {
    let mut f = fixture().await;
    for run in [&f.runs[0][0], &f.runs[0][2], &f.runs[1][0]] {
        filed(&mut f.service, "1003", join(run)).await;
    }
    assert!(matches!(
        submit(&mut f.service, "1003", join(&f.runs[1][2])).await,
        Err(RequestError::Limited(RequestLimit::Pending {
            count: 3,
            max: 3
        }))
    ));
    // Another member is not affected.
    filed(&mut f.service, "1004", join(&f.runs[1][2])).await;
}

#[tokio::test]
async fn withdrawn_and_rejected_requests_cannot_be_approved() {
    let mut f = fixture().await;
    let withdrawn = filed(&mut f.service, "1003", join(&f.runs[0][0])).await;
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
        approve(&mut f.service, &withdrawn.id, 1).await,
        Err(RequestError::Draft(DraftError::Stale {
            status: DraftStatus::Withdrawn,
            ..
        }))
    ));
    let rejected = filed(&mut f.service, "1003", join(&f.runs[0][2])).await;
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
        approve(&mut f.service, &rejected.id, 1).await,
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
    let request = filed(&mut f.service, "1003", join(&f.runs[0][0])).await;
    for actor in [Actor::member("1001"), Actor::system("delivery")] {
        assert!(matches!(
            approve_as(&mut f.service, &actor, &request.id, 1, None, &NoFreezes).await,
            Err(RequestError::Refused(RequestRefusal::NotAdmin))
        ));
        assert!(matches!(
            f.service.reject_request(&actor, &request.id, 1, "no").await,
            Err(RequestError::Refused(RequestRefusal::NotAdmin))
        ));
    }
    // The admin edits the request (joining the next week instead): v2.
    let next_week = DraftOp::SwapParticipants {
        run: Target::Existing(f.runs[0][2].clone()),
        remove: Vec::new(),
        add: vec!["1003".into()],
        via_portal: true,
    };
    let edited = f
        .service
        .edit_request(&admin(), &request.id, 1, vec![next_week], &policy(), &Guild)
        .await
        .unwrap();
    assert_eq!(edited.draft.version, 2);
    assert!(matches!(
        approve(&mut f.service, &request.id, 1).await,
        Err(RequestError::Draft(DraftError::Stale { version: 2, .. }))
    ));
    approve(&mut f.service, &request.id, 2).await.unwrap();
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
    let first = once_as(&mut f.service, join(&f.runs[0][0]), &NoFreezes)
        .await
        .unwrap();
    let again = once_as(&mut f.service, join(&f.runs[0][0]), &NoFreezes)
        .await
        .unwrap();
    assert_eq!(first.id, again.id, "the exact retry returns the request");
    assert!(matches!(
        once_as(&mut f.service, join(&f.runs[0][2]), &NoFreezes).await,
        Err(RequestError::Draft(DraftError::RequestMismatch { draft_id })) if draft_id == first.id
    ));
    let submitted = f.service.store().list_drafts(Some(DraftStatus::Submitted));
    assert_eq!(submitted.await.unwrap().len(), 1);
}

fn retime(fixed: &str) -> RequestSpec {
    RequestSpec::ChangeFixed {
        fixed_id: fixed.to_owned(),
        edit: FixedEdit {
            time: Some(hm(22, 0)),
            ..FixedEdit::default()
        },
    }
}

#[tokio::test]
async fn change_requests_need_choices_and_report_stale_ones() {
    let mut f = fixture().await;
    // Only day, time, party or channel.
    let note = RequestSpec::ChangeFixed {
        fixed_id: f.fixed[0].clone(),
        edit: FixedEdit {
            note: Some("x".into()),
            ..FixedEdit::default()
        },
    };
    assert!(matches!(
        submit(&mut f.service, "1001", note).await,
        Err(RequestError::Refused(RequestRefusal::FieldNotAllowed))
    ));
    let request = filed(&mut f.service, "1001", retime(&f.fixed[0])).await;
    assert!(
        request.scope.expires_week().is_none(),
        "a weekly change does not expire by week"
    );
    assert!(matches!(
        approve(&mut f.service, &request.id, 1).await,
        Err(RequestError::Refused(RequestRefusal::ChoicesRequired))
    ));
    // Another amendment lands upstream: the reviewed choices are stale.
    let stale = filed(&mut f.service, "1002", retime(&f.fixed[0])).await;
    let moved = run_row(&snapshot(&f.service).await, &f.runs[0][2]).datetime + TimeDelta::days(1);
    by_tests(&mut f.service)
        .amend_run(&f.runs[0][2], moved, &policy())
        .await
        .unwrap();
    let choices = keep_for_this_week(&[&f.runs[0][1], &f.runs[0][2]]);
    let result = approve_with(&mut f.service, &stale.id, Some(choices)).await;
    assert!(
        matches!(
            &result,
            Err(RequestError::Draft(DraftError::Conflicts(conflicts)))
                if conflicts.iter().any(|conflict| matches!(
                    conflict,
                    MergeConflict::ChoicesStale { .. }
                ))
        ),
        "{result:?}"
    );
    // Choices only apply to change requests.
    let joined = filed(&mut f.service, "1003", join(&f.runs[1][0])).await;
    let all = Some(FixedEditChoices::UpdateAll);
    assert!(matches!(
        approve_as(&mut f.service, &admin(), &joined.id, 1, all, &NoFreezes).await,
        Err(RequestError::Refused(RequestRefusal::ChoicesNotApplicable))
    ));
}

#[tokio::test]
async fn a_change_request_merges_with_per_run_choices() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1001", retime(&f.fixed[0])).await;
    let choices = keep_for_this_week(&[&f.runs[0][1]]);
    let before = run_row(&snapshot(&f.service).await, &f.runs[0][1]).datetime;
    approve_with(&mut f.service, &request.id, Some(choices))
        .await
        .unwrap();
    let state = snapshot(&f.service).await;
    let timing = state
        .fixed_runs
        .iter()
        .find(|row| row.id == f.fixed[0])
        .unwrap();
    assert_eq!(timing.time, hm(22, 0));
    let kept = run_row(&state, &f.runs[0][1]);
    assert_eq!(kept.datetime, before, "the amended run kept its slot");
}

#[tokio::test]
async fn run_requests_expire_after_the_reset_and_weekly_ones_do_not() {
    let mut f = fixture().await;
    let weekly = filed(&mut f.service, "1003", join_timing(&f.fixed[1])).await;
    assert!(weekly.scope.expires_week().is_none());
    let this_week = filed(&mut f.service, "1003", join(&f.runs[0][0])).await;
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
        let rows = f.service.store().outbox_notices().await.unwrap();
        let closes = rows
            .into_iter()
            .filter(|row| row.source.starts_with("draft:"));
        closes.collect::<Vec<_>>()
    };
    let outbox = closes(&f).await;
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0].source, draft_source(&this_week.id));
    assert_eq!(&outbox[0].notice, notice);
    let again = f.service.expire_due_drafts(&policy()).await.unwrap();
    assert!(again.ids.is_empty());
    assert_eq!(closes(&f).await.len(), 1);
    let still = stored(&f.service, &weekly.id).await;
    assert_eq!(still.draft.status, DraftStatus::Submitted);
    // An expired request cannot be approved.
    assert!(matches!(
        approve(&mut f.service, &this_week.id, 1).await,
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
        approve(&mut f.service, &draft.id, 1).await,
        Err(RequestError::AdminDraft)
    ));
    let request = filed(&mut f.service, "1003", join(&f.runs[0][0])).await;
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
    let timing = join_timing(&f.fixed[1]);
    let first = filed(&mut f.service, "1003", timing.clone()).await;
    let second = filed(&mut f.service, "1004", timing).await;
    approve(&mut f.service, &first.id, 1).await.unwrap();
    let result = approve(&mut f.service, &second.id, 1).await;
    assert!(result.is_ok(), "{result:?}");
    let party = party_of(&snapshot(&f.service).await, &f.fixed[1]);
    assert_eq!(party, ["1001", "1002", "1003", "1004"]);
}

#[tokio::test]
async fn a_leave_after_an_upstream_join_merges() {
    let mut f = fixture().await;
    let leave = filed(&mut f.service, "1002", leave_timing(&f.fixed[1])).await;
    let join = filed(&mut f.service, "1003", join_timing(&f.fixed[1])).await;
    approve(&mut f.service, &join.id, 1).await.unwrap();
    approve(&mut f.service, &leave.id, 1).await.unwrap();
    let state = snapshot(&f.service).await;
    assert_eq!(party_of(&state, &f.fixed[1]), ["1001", "1003"]);
    // The pushed runs follow the timing.
    for run in &f.runs[1] {
        let row = run_row(&state, run);
        assert!(!row.participants.contains(&"1002".to_owned()), "{row:?}");
    }
}

#[tokio::test]
async fn a_swap_after_an_upstream_change_merges() {
    let mut f = fixture().await;
    let swap = filed(&mut f.service, "1002", swap_timing(&f.fixed[1], "1004")).await;
    // An administrator adds 1003 to the party directly meanwhile.
    let fixed = f.fixed[1].clone();
    set_party(&mut f, &fixed, &["1001", "1002", "1003"]).await;
    approve(&mut f.service, &swap.id, 1).await.unwrap();
    assert_eq!(
        party_of(&snapshot(&f.service).await, &f.fixed[1]),
        ["1001", "1003", "1004"]
    );
}

#[tokio::test]
async fn joining_a_party_you_are_on_is_refused() {
    let mut f = fixture().await;
    for (member, spec) in [
        ("1001", join_timing(&f.fixed[0])),
        ("1001", join(&f.runs[0][0])),
        ("1002", swap_timing(&f.fixed[0], "1001")),
    ] {
        let label = format!("{member} {spec:?}");
        assert!(
            matches!(
                submit(&mut f.service, member, spec).await,
                Err(RequestError::Refused(RequestRefusal::AlreadyInParty))
            ),
            "{label}"
        );
    }
}

#[tokio::test]
async fn preview_request_analyses_and_writes_nothing() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1002", leave_timing(&f.fixed[1])).await;
    let head = f.service.store().history_head().await.unwrap();
    assert!(matches!(
        preview(&f.service, &Actor::member("1001"), &request.id, &NoFreezes).await,
        Err(RequestError::Refused(RequestRefusal::NotAdmin))
    ));
    let preview = preview(&f.service, &admin(), &request.id, &Frozen("1002"))
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
    let approved = approve_as(
        &mut f.service,
        &admin(),
        &request.id,
        1,
        None,
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
    let request = filed(&mut f.service, "1003", join(&f.runs[0][0])).await;
    for reason in ["x".repeat(501), "no\u{7}".to_owned(), "a\nb".to_owned()] {
        assert!(matches!(
            f.service
                .reject_request(&admin(), &request.id, 1, &reason)
                .await,
            Err(RequestError::Refused(RequestRefusal::ReasonInvalid))
        ));
    }
    let padded = format!("  {}  ", "y".repeat(500));
    let decided = f
        .service
        .reject_request(&admin(), &request.id, 1, &padded)
        .await
        .unwrap();
    assert_eq!(decided.request.close_reason, Some("y".repeat(500)));
}

#[tokio::test]
async fn an_exact_submit_retry_replays_before_other_checks() {
    let mut f = fixture().await;
    let first = once_as(&mut f.service, join(&f.runs[0][0]), &NoFreezes)
        .await
        .unwrap();
    let again = once_as(&mut f.service, join(&f.runs[0][0]), &Frozen("1003"))
        .await
        .unwrap();
    assert_eq!(again.id, first.id, "frozen since, the retry still replays");
}

#[tokio::test]
async fn approved_choices_are_recorded_in_the_merged_event() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1001", retime(&f.fixed[0])).await;
    let choices = Some(keep_for_this_week(&[&f.runs[0][1]]));
    let approved = approve_with(&mut f.service, &request.id, choices)
        .await
        .unwrap();
    let events = f.service.store().draft_events(&request.id).await.unwrap();
    let merged = events.last().unwrap();
    assert_eq!(merged.kind, DraftEventKind::Merged);
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
    inner: &'a SqliteStore,
    twist: Twist,
    fixed: String,
    requester: String,
    merges: AtomicUsize,
    written: AtomicBool,
}

impl Twisted<'_> {
    fn broken(&self) -> StoreResult<()> {
        if self.twist == Twist::FailLoadsAfterWrite && self.written.load(Ordering::SeqCst) {
            Err(StoreError::Backend("read after write".into()))
        } else {
            Ok(())
        }
    }

    fn wrote(&self) {
        self.written.store(true, Ordering::SeqCst);
    }
}

type StoreResult<T> = Result<T, StoreError>;

impl ScheduleStore for Twisted<'_> {
    async fn load(&self, scope: &Scope) -> StoreResult<ScheduleSnapshot> {
        self.broken()?;
        self.inner.load(scope).await
    }

    async fn recorded_request(
        &self,
        actor: &Actor,
        request_id: &str,
    ) -> StoreResult<Option<RecordedRequest>> {
        self.inner.recorded_request(actor, request_id).await
    }

    async fn commit(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
    ) -> StoreResult<Option<Committed>> {
        self.inner.commit(expected_revision, changes, meta).await
    }
}

impl DraftStore for Twisted<'_> {
    async fn snapshot_with_head(&self) -> StoreResult<(ScheduleSnapshot, ChangeRef)> {
        self.broken()?;
        self.inner.snapshot_with_head().await
    }

    async fn records_after(&self, base: &ChangeRef) -> StoreResult<Vec<ChangeRecord>> {
        self.inner.records_after(base).await
    }

    async fn create_draft(&self, new: NewDraft) -> StoreResult<DraftCreated> {
        self.inner.create_draft(new).await
    }

    async fn load_draft(&self, id: &str) -> StoreResult<Option<LoadedDraft>> {
        self.broken()?;
        self.inner.load_draft(id).await
    }

    async fn recorded_draft_request(
        &self,
        author: &Actor,
        request_id: &str,
    ) -> StoreResult<Option<(String, StoredDraft)>> {
        self.inner.recorded_draft_request(author, request_id).await
    }

    async fn list_drafts(&self, status: Option<DraftStatus>) -> StoreResult<Vec<StoredDraft>> {
        self.inner.list_drafts(status).await
    }

    async fn draft_events(&self, id: &str) -> StoreResult<Vec<DraftEvent>> {
        self.inner.draft_events(id).await
    }

    async fn update_draft(&self, update: DraftUpdate) -> StoreResult<DraftWrite> {
        let written = self.inner.update_draft(update).await?;
        self.wrote();
        Ok(written)
    }

    async fn commit_merge(
        &self,
        expected_revision: u64,
        changes: ChangeSet,
        meta: ChangeMeta,
        draft_id: &str,
        expected_version: u64,
        note: Option<String>,
    ) -> StoreResult<MergeCommit> {
        let first = self.merges.fetch_add(1, Ordering::SeqCst) == 0;
        if first && self.twist == Twist::RemoveRequester {
            let state = self.inner.load(&Scope::All).await?;
            let mut row = state
                .fixed_runs
                .iter()
                .find(|row| row.id == self.fixed)
                .unwrap()
                .clone();
            row.participants.retain(|user| *user != self.requester);
            let upstream = ChangeMeta {
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
            let changes = ChangeSet {
                changes: vec![Change::PutFixedRun(row)],
            };
            self.inner.commit(state.revision, changes, upstream).await?;
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
        week: DateTime<Utc>,
        at: DateTime<Utc>,
        actor: &Actor,
        notices: Vec<(String, Notice)>,
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
        merges: AtomicUsize::new(0),
        written: AtomicBool::new(false),
    }
}

#[tokio::test]
async fn a_requester_removed_before_the_commit_is_refused() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1002", swap_timing(&f.fixed[1], "1003")).await;
    let store = twisted(&f, Twist::RemoveRequester, "1002");
    let mut service = SchedulerService::new(&store, RandomIds, TestClock::new(kl(27, 1, 0)));
    let result = approve(&mut service, &request.id, 1).await;
    assert!(
        matches!(
            result,
            Err(RequestError::Refused(RequestRefusal::RequesterUnauthorised))
        ),
        "{result:?}"
    );
    assert_eq!(
        store.merges.load(Ordering::SeqCst),
        1,
        "the raced commit conflicted and the retry never committed"
    );
    let stored = stored(&f.service, &request.id).await;
    assert_eq!(stored.draft.status, DraftStatus::Submitted);
}

#[tokio::test]
async fn committed_decisions_never_fail_on_later_reads() {
    let mut f = fixture().await;
    let approve_id = filed(&mut f.service, "1003", join(&f.runs[0][0])).await.id;
    let reject_id = filed(&mut f.service, "1003", join(&f.runs[0][2])).await.id;
    for (id, approving) in [(&approve_id, true), (&reject_id, false)] {
        let store = twisted(&f, Twist::FailLoadsAfterWrite, "1003");
        let mut service = SchedulerService::new(&store, RandomIds, TestClock::new(kl(27, 1, 0)));
        let notice = if approving {
            approve(&mut service, id, 1).await.unwrap().requester_notice
        } else {
            let rejected = service.reject_request(&admin(), id, 1, "no room").await;
            rejected.unwrap().requester_notice
        };
        assert_eq!(notice.listed, ["1003"]);
        assert!(store.written.load(Ordering::SeqCst));
    }
}

#[tokio::test]
async fn a_timing_join_merges_after_an_upstream_one_off_substitution() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1003", join_timing(&f.fixed[1])).await;
    let run = f.runs[1][0].clone();
    substitute(&mut f, &run, &["1002"], &["1004"]).await;
    approve(&mut f.service, &request.id, 1).await.unwrap();
    let state = snapshot(&f.service).await;
    let party = sorted(run_row(&state, &run).participants);
    assert_eq!(party, ["1001", "1003", "1004"], "the substitute stays");
    assert_eq!(party_of(&state, &f.fixed[1]), ["1001", "1002", "1003"]);
}

#[tokio::test]
async fn a_substitution_made_before_submit_survives_approval() {
    let mut f = fixture().await;
    let run = f.runs[1][0].clone();
    substitute(&mut f, &run, &["1002"], &["1004"]).await;
    answer(&mut f, &run, "1004").await;
    let request = filed(&mut f.service, "1003", join_timing(&f.fixed[1])).await;
    approve(&mut f.service, &request.id, 1).await.unwrap();
    let state = snapshot(&f.service).await;
    let party = sorted(run_row(&state, &run).participants);
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
    substitute(&mut f, &middle, &["1001"], &["1004"]).await;
    let request = filed(&mut f.service, "1002", leave_timing(&f.fixed[1])).await;
    approve(&mut f.service, &request.id, 1).await.unwrap();
    let state = snapshot(&f.service).await;
    for run in &runs {
        assert_eq!(answered(&state, run), ["1001"], "{run}");
        let party = run_row(&state, run).participants;
        assert!(!party.contains(&"1002".to_owned()), "{run}: {party:?}");
    }
    assert_eq!(run_row(&state, &middle).participants, ["1004"]);
}

#[tokio::test]
async fn a_request_already_in_effect_is_flagged_not_closed() {
    let mut f = fixture().await;
    let request = filed(&mut f.service, "1003", join_timing(&f.fixed[1])).await;
    // An administrator adds 1003 directly meanwhile.
    let fixed = f.fixed[1].clone();
    set_party(&mut f, &fixed, &["1001", "1002", "1003"]).await;
    let preview_now = preview(&f.service, &admin(), &request.id, &NoFreezes)
        .await
        .unwrap();
    assert!(
        preview_now.no_effect,
        "{:?}",
        preview_now.analysis.conflicts
    );
    let head = f.service.store().history_head().await.unwrap();
    assert!(matches!(
        approve(&mut f.service, &request.id, 1).await,
        Err(RequestError::NoEffect)
    ));
    assert_eq!(f.service.store().history_head().await.unwrap(), head);
    assert_eq!(
        stored(&f.service, &request.id).await.draft.status,
        DraftStatus::Submitted,
        "no automatic close"
    );
    // A fresh request is not flagged.
    let fresh = filed(&mut f.service, "1004", join_timing(&f.fixed[1])).await;
    let fresh = preview(&f.service, &admin(), &fresh.id, &NoFreezes).await;
    assert!(!fresh.unwrap().no_effect);
}

#[tokio::test]
async fn a_delta_that_would_empty_a_run_skips_it_and_warns() {
    let mut f = fixture().await;
    let lonely = f.runs[1][0].clone();
    // This week only 1002 plays that run.
    substitute(&mut f, &lonely, &["1001"], &[]).await;
    let request = filed(&mut f.service, "1002", leave_timing(&f.fixed[1])).await;
    let skipped = [MergeWarning::PartyDeltaSkipped {
        run_id: lonely.clone(),
        reason: SkipReason::Emptied,
    }];
    // The preview shows the run the approval would skip.
    let preview = preview(&f.service, &admin(), &request.id, &NoFreezes)
        .await
        .unwrap();
    assert_eq!(preview.warnings, skipped);
    let approved = approve(&mut f.service, &request.id, 1).await.unwrap();
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
