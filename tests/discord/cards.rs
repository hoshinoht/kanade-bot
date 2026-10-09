//! Proposal cards end to end on the fake transport and the memory store:
//! journalled posting (claim → send → bind; ambiguous sends held, never
//! replayed; refused sends reposted with the next card), ✅/❌ through the
//! scheduler's approval rules, card refreshes and chat answers; the
//! redesigned embed (`styled`) and Components V2 card with its buttons
//! (`v2`).

mod replay;
mod styled;
mod v2;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, NaiveTime, TimeZone, Utc, Weekday};
use kanade::api::state::ProposalCardRefresh;
use kanade::bot::cards::CardOutbox;
use kanade::bot::cards::{Authority, CardDesk, CardReaction, CardSettings, DeskDeps};
use kanade::bot::delivery::AdminAlert;
use kanade::bot::delivery::{AlertRecorder, FixedClock, StoreRef};
use kanade::bot::events::RsvpAnswer;
use kanade::bot::transport::{
    AmbiguousKind, Call, ChannelId, DiscordTransport, FakeDiscord, InteractionRef,
    InteractionReply, MessageEdit, MessageId, Op, Outcome, OutgoingMessage, Presence,
    RejectionKind, Step,
};
use kanade::chat::persona::NudgePurpose;
use kanade::domain::drafts::{DraftStatus, ProposalSource, ProposalStore};
use kanade::domain::history::Origin;
use kanade::domain::history::{Actor, Surface};
use kanade::domain::ids::IdGenerator;
use kanade::domain::members::{Directory, Member};
use kanade::domain::notify::{DeclineNoticeStore, DeliveryJournal};
use kanade::domain::proposals::{Approver, ChangeKind, ProposalCardStore, ProposedChange};
use kanade::domain::schedule::RsvpSource;
use kanade::domain::schedule::{
    NewRun, ReminderPolicy, RsvpState, RunSource, RunStatus, SchedulePolicy,
};
use kanade::domain::scheduler::{
    ProposalRequest, ScheduleStore, SchedulerService, Scope, Supersede, SupersedeScope,
};
use kanade::extract::AmendmentKind;
use kanade::extract::pipeline::{
    BacklogDrop, Card, CardEntry, ChatAnswer, Outbox, PostResult, Redirected, SelfServiceTip,
};
use kanade::extract::redirect::RedirectLink;
use kanade::infrastructure::store::MemoryScheduleStore;
use twilight_model::application::command::Command;
use twilight_model::id::{Id, marker::GuildMarker};

const CHANNEL: &str = "300";
const MY: &str = "1001";
const ALVIN: &str = "1002";
const STRANGER: &str = "1009";
const ADMIN: &str = "1003";

fn zone() -> chrono_tz::Tz {
    chrono_tz::Asia::Kuala_Lumpur
}

fn local(month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    zone()
        .with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .single()
        .expect("instant")
        .with_timezone(&Utc)
}

fn now() -> DateTime<Utc> {
    local(8, 30, 13, 7)
}

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: zone(),
            ping_time: NaiveTime::from_hms_opt(18, 0, 0).expect("time"),
            countdowns: Vec::new(),
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

#[derive(Clone, Default)]
struct Ids(Arc<AtomicUsize>);

impl IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        format!("c0c0c0c0-0000-4000-8000-{n:012}")
    }
}

struct Roster;

impl Directory for Roster {
    fn member(&self, user_id: &str) -> Option<Member> {
        let name = match user_id {
            MY => "Mylene",
            ALVIN => "Alvin",
            ADMIN => "Boss",
            _ => return None,
        };
        Some(Member {
            user_id: user_id.into(),
            display_name: Some(name.into()),
            has_role: true,
            ..Member::default()
        })
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

struct Staff;

impl Authority for Staff {
    fn approver(&self, user_id: &str) -> Approver {
        Approver {
            user_id: user_id.into(),
            has_role: Roster.member(user_id).is_some(),
            is_admin: user_id == ADMIN,
            via_portal: false,
        }
    }
}

type Desk = CardDesk<MemoryScheduleStore, FakeDiscord, Ids, AlertRecorder>;

struct World {
    store: Arc<MemoryScheduleStore>,
    discord: Arc<FakeDiscord>,
    alerts: Arc<AlertRecorder>,
    ids: Ids,
    desk: Desk,
    run: String,
}

fn desk(
    store: &Arc<MemoryScheduleStore>,
    discord: &Arc<FakeDiscord>,
    alerts: &Arc<AlertRecorder>,
    ids: &Ids,
) -> Desk {
    desk_at(store, discord, alerts, ids, now())
}

fn desk_at(
    store: &Arc<MemoryScheduleStore>,
    discord: &Arc<FakeDiscord>,
    alerts: &Arc<AlertRecorder>,
    ids: &Ids,
    at: DateTime<Utc>,
) -> Desk {
    CardDesk::new(
        DeskDeps {
            store: store.clone(),
            transport: discord.clone(),
            ids: ids.clone(),
            clock: Arc::new(FixedClock(at)),
            directory: Arc::new(Roster),
            authority: Arc::new(Staff),
            alerts: alerts.clone(),
            decline_retraction: None,
        },
        CardSettings {
            zone: zone(),
            policy: policy(),
            instance_id: "instance-1".into(),
        },
    )
}

impl World {
    async fn new() -> Self {
        let store = Arc::new(MemoryScheduleStore::new());
        let discord = Arc::new(FakeDiscord::new());
        let alerts = Arc::new(AlertRecorder::new());
        let ids = Ids::default();
        let run = service(&store, &ids)
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: None,
                channel_id: Some(CHANNEL.into()),
                week_start: local(8, 27, 0, 0),
                datetime: local(8, 31, 21, 30),
                bosses: vec!["HMaleficStar".into(), "HFA".into()],
                participants: vec![MY.into(), ALVIN.into()],
                status: RunStatus::Planned,
                source: RunSource::Amend,
            })
            .await
            .expect("run");
        let desk = desk(&store, &discord, &alerts, &ids);
        Self {
            store,
            discord,
            alerts,
            ids,
            desk,
            run,
        }
    }

    /// A proposed move of the run and its card entry.
    async fn propose(&self, to: DateTime<Utc>) -> CardEntry {
        self.propose_from(to, ProposalSource::Extraction).await
    }

    async fn propose_from(&self, to: DateTime<Utc>, source: ProposalSource) -> CardEntry {
        let change = ProposedChange {
            run_id: Some(self.run.clone()),
            channel_id: Some(CHANNEL.into()),
            bosses: vec!["HMaleficStar".into(), "HFA".into()],
            participants: vec![MY.into()],
            new_datetime: Some(to),
            ..ProposedChange::new(ChangeKind::Move)
        };
        let proposed = service(&self.store, &self.ids)
            .propose(
                ProposalRequest {
                    change: change.clone(),
                    source,
                    source_id: "x-1".into(),
                    supersede: Supersede::Keep,
                },
                &policy(),
                &Roster,
            )
            .await
            .expect("proposal");
        CardEntry {
            proposal_id: proposed.proposal.id,
            change,
            kind: AmendmentKind::Move,
            run_id: Some(self.run.clone()),
            summary: "moving to wed".into(),
            is_question: false,
            needs_answer: false,
            confidence: 0.9,
            also_mentioned: Vec::new(),
            day_ref: Some("wed".into()),
            time_ref: Some("9:30pm".into()),
            evidence_message_ids: vec!["101".into()],
            self_service: None,
        }
    }

    fn creates(&self) -> Vec<Call> {
        self.discord
            .calls()
            .into_iter()
            .filter(|call| call.op() == Op::Create)
            .collect()
    }

    fn last_edit_content(&self) -> String {
        self.discord
            .calls()
            .into_iter()
            .rev()
            .find_map(|call| match call {
                Call::Edit { edit, .. } => edit.content,
                _ => None,
            })
            .expect("an edit")
    }

    async fn message_of(&self, proposal: &str) -> Option<String> {
        self.store
            .load_cards(&[proposal.to_owned()])
            .await
            .expect("cards")
            .pop()
            .and_then(|card| card.message_id)
    }

    async fn status(&self, proposal: &str) -> DraftStatus {
        self.store
            .load_proposal(proposal)
            .await
            .expect("load")
            .expect("proposal")
            .0
            .draft
            .status
    }
}

fn service<'a>(
    store: &'a Arc<MemoryScheduleStore>,
    ids: &Ids,
) -> SchedulerService<StoreRef<'a, MemoryScheduleStore>, Ids, FixedClock> {
    SchedulerService::new(StoreRef(&**store), ids.clone(), FixedClock(now()))
}

fn card(entries: Vec<CardEntry>, superseded: Vec<String>) -> Card {
    Card {
        channel_id: CHANNEL.into(),
        entries,
        superseded,
    }
}

#[tokio::test]
async fn a_card_is_journalled_bound_and_mentions_nobody() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    assert_eq!(
        world.desk.post_card(&card(vec![entry], Vec::new())).await,
        PostResult::Posted
    );

    let creates = world.creates();
    assert_eq!(creates.len(), 1);
    let Call::Create {
        message, outcome, ..
    } = &creates[0]
    else {
        unreachable!()
    };
    assert!(
        message.allowed_mentions.users.is_empty(),
        "nobody is pinged"
    );
    let content = message.content.as_deref().unwrap_or_default();
    assert_eq!(content, "📋 Proposed change\nMylene");
    let field = &message.embeds[0].fields[0];
    assert!(
        field
            .name
            .starts_with("move · Hard MaleficStar + Hard FA · `#")
    );
    assert_eq!(
        field.value,
        "~~Mon 31 Aug 21:30~~ → **Wed 02 Sep 21:30**\nMylene\n_moving to wed_"
    );
    let Outcome::Delivered(posted) = outcome else {
        panic!("delivered");
    };
    let posted = posted.get().to_string();
    assert_eq!(world.message_of(&id).await, Some(posted));
    assert_eq!(world.discord.count(Op::AddReaction), 2, "✅ and ❌");
}

#[tokio::test]
async fn an_ambiguous_card_is_held_and_never_replayed() {
    let world = World::new().await;
    world.discord.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    let first = world.propose(local(9, 2, 21, 30)).await;
    assert_eq!(
        world.desk.post_card(&card(vec![first], Vec::new())).await,
        PostResult::Posted
    );
    let second = world.propose(local(9, 2, 22, 30)).await;
    world.desk.post_card(&card(vec![second], Vec::new())).await;
    assert_eq!(
        world.creates().len(),
        2,
        "only the new card; the held one is not resent"
    );
}

#[tokio::test]
async fn a_refused_card_is_not_posted_and_rides_with_the_next_one() {
    let world = World::new().await;
    world
        .discord
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let first = world.propose(local(9, 2, 21, 30)).await;
    let first_id = first.proposal_id.clone();
    assert_eq!(
        world.desk.post_card(&card(vec![first], Vec::new())).await,
        PostResult::Pending,
        "saved and reposted later, so its tips stay spent"
    );
    assert!(
        !world.alerts.alerts().is_empty(),
        "a refused send is alerted"
    );
    let second = world.propose(local(9, 2, 22, 30)).await;
    world.desk.post_card(&card(vec![second], Vec::new())).await;
    assert_eq!(
        world.creates().len(),
        3,
        "the stranded card is posted first (v4)"
    );
    assert!(world.message_of(&first_id).await.is_some());
}

#[tokio::test]
async fn a_participant_approves_and_the_card_says_so() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");

    assert_eq!(
        world
            .desk
            .on_reaction(&message, STRANGER, RsvpAnswer::Yes, true)
            .await,
        CardReaction::Ignored,
        "not theirs to answer: silence"
    );
    assert_eq!(world.discord.count(Op::Edit), 0);
    assert_eq!(
        world
            .desk
            .on_reaction(&message, MY, RsvpAnswer::Yes, false)
            .await,
        CardReaction::Ignored,
        "removing a reaction does nothing"
    );
    let CardReaction::Approved { approved, problems } = world
        .desk
        .on_reaction(&message, MY, RsvpAnswer::Yes, true)
        .await
    else {
        panic!("approved");
    };
    assert_eq!(approved.len(), 1);
    assert!(problems.is_empty());
    assert_eq!(world.status(&id).await, DraftStatus::Merged);
    // The merge's notices were written to the outbox with it.
    let outbox = || async {
        kanade::domain::notify::NoticeOutbox::outbox_notices(&*world.store)
            .await
            .expect("outbox")
    };
    let written = outbox().await;
    assert!(!written.is_empty());
    assert!(
        written.iter().all(|row| {
            row.source == kanade::domain::notify::change_source(approved[0].merge.seq)
        })
    );
    assert_eq!(
        written
            .into_iter()
            .map(|row| row.notice)
            .collect::<Vec<_>>(),
        approved[0].merge.notices
    );
    let run = world
        .store
        .load(&Scope::Run(world.run.clone()))
        .await
        .expect("run")
        .runs
        .remove(0);
    assert_eq!(run.datetime, local(9, 2, 21, 30));
    assert!(
        world
            .last_edit_content()
            .ends_with("\n✅ applied by Mylene")
    );
    assert_eq!(
        world
            .desk
            .on_reaction(&message, MY, RsvpAnswer::Yes, true)
            .await,
        CardReaction::Ignored,
        "a repeated ✅ writes and posts nothing"
    );
    assert_eq!(outbox().await.len(), approved[0].merge.notices.len());
}

#[tokio::test]
async fn an_admin_rejects_and_the_card_says_so() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    assert_eq!(
        world
            .desk
            .on_reaction(&message, ADMIN, RsvpAnswer::No, true)
            .await,
        CardReaction::Rejected {
            proposal_ids: vec![id.clone()]
        }
    );
    assert_eq!(world.status(&id).await, DraftStatus::Rejected);
    assert!(world.last_edit_content().ends_with("\n❌ rejected by Boss"));
    assert!(
        world
            .desk
            .rejection_follow_up(std::slice::from_ref(&id))
            .await
            .is_none(),
        "extraction cards never start chat follow-ups"
    );
    assert_eq!(
        world
            .desk
            .on_reaction(&message, ADMIN, RsvpAnswer::No, true)
            .await,
        CardReaction::Ignored,
        "the closed card cannot follow up twice"
    );
}

#[tokio::test]
async fn a_successfully_rejected_chat_card_hands_only_stored_facts_to_chat() {
    let world = World::new().await;
    let entry = world
        .propose_from(local(9, 2, 21, 30), ProposalSource::Chat)
        .await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    assert!(matches!(
        world
            .desk
            .on_reaction(&message, MY, RsvpAnswer::No, true)
            .await,
        CardReaction::Rejected { .. }
    ));
    let follow_up = world
        .desk
        .rejection_follow_up(&[id])
        .await
        .expect("chat source");
    assert_eq!(follow_up.channel_id, CHANNEL);
    assert_eq!(follow_up.source_ids, ["x-1"]);
    assert_eq!(follow_up.cards[0].summary.as_deref(), Some("moving to wed"));
    assert_eq!(follow_up.cards[0].participants, [MY]);
}

#[tokio::test]
async fn an_approval_that_no_longer_applies_keeps_v4_wording() {
    let world = World::new().await;
    // A carded answer for Alvin (the chatbot cards these), checked again at ✅.
    let change = ProposedChange {
        run_id: Some(world.run.clone()),
        channel_id: Some(CHANNEL.into()),
        participants: vec![ALVIN.into()],
        rsvp: Some(RsvpState::Yes),
        ..ProposedChange::new(ChangeKind::Rsvp)
    };
    let proposed = service(&world.store, &world.ids)
        .propose(
            ProposalRequest {
                change: change.clone(),
                source: ProposalSource::Extraction,
                source_id: "x-2".into(),
                supersede: Supersede::Keep,
            },
            &policy(),
            &Roster,
        )
        .await
        .expect("proposal");
    let id = proposed.proposal.id;
    let entry = CardEntry {
        proposal_id: id.clone(),
        change,
        kind: AmendmentKind::Rsvp,
        run_id: Some(world.run.clone()),
        summary: String::new(),
        is_question: false,
        needs_answer: false,
        confidence: 0.9,
        also_mentioned: Vec::new(),
        day_ref: None,
        time_ref: None,
        evidence_message_ids: Vec::new(),
        self_service: None,
    };
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    service(&world.store, &world.ids)
        .as_origin(Origin::for_tests())
        .swap_participants(&world.run, &[ALVIN.into()], &[], false, &Roster)
        .await
        .expect("Alvin left by hand");
    let refusal = "that answer is for somebody who is no longer on the run";
    assert_eq!(
        world
            .desk
            .on_reaction(&message, MY, RsvpAnswer::Yes, true)
            .await,
        CardReaction::Approved {
            approved: Vec::new(),
            problems: vec![refusal.into()],
        }
    );
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    let Some(Call::Create { message, .. }) = world.creates().pop() else {
        panic!("a notice");
    };
    assert_eq!(message.content, Some(format!("⚠️ {refusal}")));
    assert!(message.allowed_mentions.users.is_empty());
}

#[tokio::test]
async fn a_newer_card_marks_the_retired_one_superseded() {
    let world = World::new().await;
    let first = world.propose(local(9, 2, 21, 30)).await;
    let first_id = first.proposal_id.clone();
    world.desk.post_card(&card(vec![first], Vec::new())).await;
    // The pipeline retires older proposals through the scheduler first.
    let retired = service(&world.store, &world.ids)
        .supersede_proposals(SupersedeScope {
            run_id: Some(&world.run),
            channel_id: Some(CHANNEL),
            bosses: &[],
            keep: None,
            from_channel: Some(CHANNEL),
            by: ProposalSource::Extraction,
        })
        .await
        .expect("supersede");
    assert_eq!(retired, std::slice::from_ref(&first_id));
    let second = world.propose(local(9, 2, 22, 30)).await;
    world.desk.post_card(&card(vec![second], retired)).await;
    assert_eq!(world.discord.count(Op::Edit), 1);
    assert!(
        world
            .last_edit_content()
            .ends_with("\n↪ superseded by a newer card")
    );
}

#[tokio::test]
async fn a_restarted_desk_re_renders_a_card_from_its_stored_details() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    let restarted = desk(&world.store, &world.discord, &world.alerts, &world.ids);
    assert!(restarted.refresh(&message).await);
    let Some(Call::Edit { edit, .. }) = world.discord.calls().pop() else {
        panic!("an edit");
    };
    assert_eq!(edit.content.as_deref(), Some("📋 Proposed change\nMylene"));
    assert!(
        edit.embeds.expect("embed")[0].fields[0]
            .value
            .contains("_moving to wed_")
    );
}

#[tokio::test]
async fn chat_answers_count_for_participants_only() {
    let world = World::new().await;
    let answers = [ChatAnswer {
        channel_id: CHANNEL.into(),
        run_id: world.run.clone(),
        user_ids: vec![ALVIN.into(), STRANGER.into()],
        state: RsvpState::Yes,
    }];
    assert_eq!(world.desk.apply_answers(&answers).await, 1);
    let state = world
        .store
        .load(&Scope::Run(world.run.clone()))
        .await
        .expect("run");
    let rsvps: BTreeMap<&str, (RsvpState, &str)> = state
        .rsvps
        .iter()
        .map(|rsvp| (rsvp.user_id.as_str(), (rsvp.state, rsvp.source.as_str())))
        .collect();
    assert_eq!(rsvps.get(ALVIN), Some(&(RsvpState::Yes, "chat")));
    assert!(!rsvps.contains_key(STRANGER));
}

#[tokio::test]
async fn proposal_decline_is_not_a_candidate_but_extraction_decline_is() {
    let proposal_world = World::new().await;
    let change = ProposedChange {
        run_id: Some(proposal_world.run.clone()),
        channel_id: Some(CHANNEL.into()),
        participants: vec![MY.into()],
        rsvp: Some(RsvpState::No),
        ..ProposedChange::new(ChangeKind::Rsvp)
    };
    let proposed = service(&proposal_world.store, &proposal_world.ids)
        .propose(
            ProposalRequest {
                change: change.clone(),
                source: ProposalSource::Chat,
                source_id: "chat-no-1".into(),
                supersede: Supersede::Keep,
            },
            &policy(),
            &Roster,
        )
        .await
        .expect("proposal");
    let proposal_id = proposed.proposal.id;
    let entry = CardEntry {
        proposal_id: proposal_id.clone(),
        change,
        kind: AmendmentKind::Rsvp,
        run_id: Some(proposal_world.run.clone()),
        summary: "I can't make it".into(),
        is_question: false,
        needs_answer: false,
        confidence: 0.9,
        also_mentioned: Vec::new(),
        day_ref: None,
        time_ref: None,
        evidence_message_ids: vec!["101".into()],
        self_service: None,
    };
    proposal_world
        .desk
        .post_card(&card(vec![entry], Vec::new()))
        .await;
    let message = proposal_world
        .message_of(&proposal_id)
        .await
        .expect("posted proposal");
    assert!(matches!(
        proposal_world
            .desk
            .on_reaction(&message, MY, RsvpAnswer::Yes, true)
            .await,
        CardReaction::Approved { .. }
    ));
    assert!(
        proposal_world
            .store
            .decline_notice(&proposal_world.run, MY)
            .await
            .expect("decline notice")
            .is_none()
    );
    assert!(
        proposal_world
            .store
            .pending_decline_notices(10)
            .await
            .expect("pending declines")
            .is_empty()
    );
    assert_eq!(
        proposal_world.discord.count(Op::Create),
        1,
        "only the proposal card was posted"
    );

    let extraction_world = World::new().await;
    let outbox = CardOutbox(Arc::new(desk(
        &extraction_world.store,
        &extraction_world.discord,
        &extraction_world.alerts,
        &extraction_world.ids,
    )));
    Outbox::answers(
        &outbox,
        vec![ChatAnswer {
            channel_id: "301".into(),
            run_id: extraction_world.run.clone(),
            user_ids: vec![ALVIN.into()],
            state: RsvpState::No,
        }],
    )
    .await;
    let candidate = extraction_world
        .store
        .decline_notice(&extraction_world.run, ALVIN)
        .await
        .expect("decline notice")
        .expect("extraction decline candidate");
    assert_eq!(candidate.channel_id.as_deref(), Some("301"));
    assert_eq!(
        extraction_world
            .store
            .pending_decline_notices(10)
            .await
            .expect("pending declines")
            .len(),
        1
    );
}

#[tokio::test]
async fn the_outbox_posts_links_unpinged_and_alerts_backlog_drops() {
    let world = World::new().await;
    let desk = Arc::new(desk(
        &world.store,
        &world.discord,
        &world.alerts,
        &world.ids,
    ));
    let outbox = CardOutbox(desk);
    let redirected = Redirected {
        channel_id: CHANNEL.into(),
        change: ProposedChange::new(ChangeKind::Move),
        author_id: MY.into(),
        tip: SelfServiceTip {
            link: RedirectLink {
                purpose: NudgePurpose::SelfService,
                url: "https://portal.example/runs/r?move_to=x".into(),
                view_only: false,
            },
            lead_in: Some("Do it yourself!".into()),
            line: None,
            claimed: None,
        },
    };
    assert_eq!(outbox.redirect(redirected).await, PostResult::Posted);
    let Some(Call::Create { message, .. }) = world.creates().pop() else {
        panic!("a link");
    };
    assert_eq!(
        message.content.as_deref(),
        Some("<@1001> Do it yourself! → edit the run: https://portal.example/runs/r?move_to=x")
    );
    assert!(message.allowed_mentions.users.is_empty());
    assert_eq!(world.discord.count(Op::AddReaction), 0);

    outbox
        .backlog_dropped(BacklogDrop {
            message_ids: vec!["1".into(), "2".into()],
            capacity: 2,
        })
        .await;
    assert!(world.alerts.alerts().contains(&AdminAlert::BacklogDropped {
        messages: 2,
        capacity: 2
    }));
}

#[tokio::test]
async fn a_released_card_posts_even_beside_a_held_one() {
    let world = World::new().await;
    world.discord.script(
        Op::Create,
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    );
    let held = world.propose(local(9, 2, 21, 30)).await;
    let held_id = held.proposal_id.clone();
    world.desk.post_card(&card(vec![held], Vec::new())).await;
    world
        .discord
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let released = world.propose(local(9, 2, 22, 0)).await;
    let released_id = released.proposal_id.clone();
    world
        .desk
        .post_card(&card(vec![released], Vec::new()))
        .await;
    let fresh = world.propose(local(9, 2, 22, 30)).await;
    world.desk.post_card(&card(vec![fresh], Vec::new())).await;

    assert!(world.message_of(&released_id).await.is_some(), "reposted");
    assert_eq!(world.message_of(&held_id).await, None, "held, never resent");
    assert_eq!(world.creates().len(), 4);
}

#[tokio::test]
async fn a_stranded_card_past_its_ttl_is_not_reposted() {
    let world = World::new().await;
    world
        .discord
        .script(Op::Create, Step::Reject(RejectionKind::MissingPermissions));
    let first = world.propose(local(9, 2, 21, 30)).await;
    let first_id = first.proposal_id.clone();
    world.desk.post_card(&card(vec![first], Vec::new())).await;
    let later = desk_at(
        &world.store,
        &world.discord,
        &world.alerts,
        &world.ids,
        now() + chrono::TimeDelta::hours(25),
    );
    let second = world.propose(local(9, 2, 22, 30)).await;
    later.post_card(&card(vec![second], Vec::new())).await;
    assert_eq!(world.creates().len(), 2, "only the new card");
    assert_eq!(world.message_of(&first_id).await, None);
}

#[tokio::test]
async fn a_strangers_reaction_on_a_past_ttl_card_changes_and_posts_nothing() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    let later = desk_at(
        &world.store,
        &world.discord,
        &world.alerts,
        &world.ids,
        now() + chrono::TimeDelta::hours(25),
    );
    let calls = world.discord.calls().len();
    for answer in [RsvpAnswer::Yes, RsvpAnswer::No] {
        assert_eq!(
            later.on_reaction(&message, STRANGER, answer, true).await,
            CardReaction::Ignored
        );
    }
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert_eq!(world.discord.calls().len(), calls, "no post, no edit");
}

#[tokio::test]
async fn a_hand_edited_run_leaves_the_card_out_of_date() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    service(&world.store, &world.ids)
        .as_origin(Origin::for_tests())
        .amend_run(&world.run, local(9, 1, 22, 0), &policy())
        .await
        .expect("moved by hand");
    let reaction = world
        .desk
        .on_reaction(&message, MY, RsvpAnswer::Yes, true)
        .await;
    assert!(matches!(
        reaction,
        CardReaction::Approved { ref approved, .. } if approved.is_empty()
    ));
    let posted = world.creates().pop().expect("a notice");
    let Call::Create {
        message: notice, ..
    } = posted
    else {
        unreachable!()
    };
    assert_eq!(
        notice.content.as_deref(),
        Some(
            "⚠️ That run was changed after this card went up, so I didn't apply it. \
             Check the run and ask again if it still needs changing."
        )
    );
    assert!(world.last_edit_content().ends_with("\n⚠️ out of date"));
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
}

#[tokio::test]
async fn a_failure_members_must_not_see_is_alerted_not_posted() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    // The member's approval request id already names another change.
    service(&world.store, &world.ids)
        .as_origin(
            Origin::new(Actor::member(MY), Surface::Discord)
                .with_request_id(format!("approve:{id}")),
        )
        .set_rsvp(&world.run, MY, RsvpState::Yes, RsvpSource::Reaction)
        .await
        .expect("unrelated change");
    let calls = world.discord.calls().len();
    assert_eq!(
        world
            .desk
            .on_reaction(&message, MY, RsvpAnswer::Yes, true)
            .await,
        CardReaction::Ignored
    );
    assert_eq!(world.discord.calls().len(), calls, "nothing public");
    assert!(world.alerts.alerts().iter().any(|alert| matches!(
        alert,
        AdminAlert::CardAnswerFailed { proposal_id, .. } if *proposal_id == id
    )));
}

#[tokio::test]
async fn a_members_check_after_their_own_portal_edit_stays_silent() {
    let world = World::new().await;
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = world.message_of(&id).await.expect("posted");
    // The admin approves from the portal at another time; the card is not
    // refreshed (serve composition does that).
    let admin = Staff.approver(ADMIN);
    let edited = local(9, 2, 20, 0);
    service(&world.store, &world.ids)
        .approve_proposal_at(&id, &admin, Some(edited), &policy(), &Roster)
        .await
        .expect("portal approval");
    let before = world.store.load(&Scope::All).await.expect("state");
    let calls = world.discord.calls().len();
    assert_eq!(
        world
            .desk
            .on_reaction(&message, ADMIN, RsvpAnswer::Yes, true)
            .await,
        CardReaction::Ignored
    );
    assert_eq!(
        world.discord.calls().len(),
        calls,
        "nothing posted or edited"
    );
    assert!(world.alerts.alerts().is_empty(), "no fault alert");
    let after = world.store.load(&Scope::All).await.expect("state");
    assert_eq!(after, before, "nothing applied twice");
    let run = after
        .runs
        .iter()
        .find(|run| run.id == world.run)
        .expect("run");
    assert_eq!(run.datetime, edited);
}

#[tokio::test]
async fn the_portal_refresh_port_re_renders_a_committed_card_on_fake_discord() {
    let world = World::new().await;
    let desk = Arc::new(desk(
        &world.store,
        &world.discord,
        &world.alerts,
        &world.ids,
    ));
    let entry = world.propose(local(9, 2, 21, 30)).await;
    let id = entry.proposal_id.clone();
    desk.post_card(&card(vec![entry], Vec::new())).await;
    let admin = Staff.approver(ADMIN);
    service(&world.store, &world.ids)
        .approve_proposal(&id, &admin, &policy(), &Roster)
        .await
        .expect("portal approval");
    let refresh: ProposalCardRefresh = {
        let desk = Arc::clone(&desk);
        Arc::new(move |proposal_ids| {
            let desk = Arc::clone(&desk);
            Box::pin(async move { desk.refresh_proposals(&proposal_ids).await })
        })
    };
    refresh(vec![id]).await;
    assert!(world.last_edit_content().ends_with("\n✅ applied by Boss"));
}

/// `FakeDiscord` whose creates orphan every live lease first (a restart
/// recovery racing the send), so the journal write after the send fails.
struct LeaseLost {
    fake: Arc<FakeDiscord>,
    store: Arc<MemoryScheduleStore>,
}

impl DiscordTransport for LeaseLost {
    async fn create_message(
        &self,
        channel: ChannelId,
        message: &OutgoingMessage,
    ) -> Outcome<MessageId> {
        let outcome = self.fake.create_message(channel, message).await;
        self.store.recover_on_start(now()).await.expect("recover");
        outcome
    }

    fn edit_message(
        &self,
        channel: ChannelId,
        message: MessageId,
        edit: &MessageEdit,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.edit_message(channel, message, edit)
    }

    fn delete_message(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.delete_message(channel, message)
    }

    fn add_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.add_own_reaction(channel, message, emoji)
    }

    fn remove_own_reaction(
        &self,
        channel: ChannelId,
        message: MessageId,
        emoji: &str,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.remove_own_reaction(channel, message, emoji)
    }

    fn message_presence(
        &self,
        channel: ChannelId,
        message: MessageId,
    ) -> impl std::future::Future<Output = Outcome<Presence>> + Send {
        self.fake.message_presence(channel, message)
    }

    fn respond(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.respond(interaction, reply)
    }

    fn defer(
        &self,
        interaction: &InteractionRef,
        ephemeral: bool,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.defer(interaction, ephemeral)
    }

    fn complete_deferred(
        &self,
        interaction: &InteractionRef,
        reply: &InteractionReply,
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.complete_deferred(interaction, reply)
    }

    fn register_guild_commands(
        &self,
        guild: Id<GuildMarker>,
        commands: &[Command],
    ) -> impl std::future::Future<Output = Outcome<()>> + Send {
        self.fake.register_guild_commands(guild, commands)
    }

    fn list_members(
        &self,
        guild: Id<GuildMarker>,
        after: Option<Id<twilight_model::id::marker::UserMarker>>,
        limit: u16,
    ) -> impl std::future::Future<Output = Outcome<Vec<twilight_model::guild::Member>>> + Send {
        self.fake.list_members(guild, after, limit)
    }

    fn channel_messages(
        &self,
        channel: ChannelId,
        page: kanade::bot::transport::HistoryPage,
        limit: u16,
    ) -> impl std::future::Future<Output = Outcome<Vec<twilight_model::channel::Message>>> + Send
    {
        self.fake.channel_messages(channel, page, limit)
    }

    fn guild_channels(
        &self,
        guild: Id<GuildMarker>,
    ) -> impl std::future::Future<Output = Outcome<Vec<twilight_model::channel::Channel>>> + Send
    {
        self.fake.guild_channels(guild)
    }
}

#[tokio::test]
async fn a_send_whose_journal_write_failed_still_counts_as_posted() {
    for step in [
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: true,
        },
        Step::Succeed,
    ] {
        let world = World::new().await;
        world.discord.script(Op::Create, step);
        let desk = CardDesk::new(
            DeskDeps {
                store: world.store.clone(),
                transport: Arc::new(LeaseLost {
                    fake: world.discord.clone(),
                    store: world.store.clone(),
                }),
                ids: world.ids.clone(),
                clock: Arc::new(FixedClock(now())),
                directory: Arc::new(Roster),
                authority: Arc::new(Staff),
                alerts: world.alerts.clone(),
                decline_retraction: None,
            },
            CardSettings {
                zone: zone(),
                policy: policy(),
                instance_id: "instance-1".into(),
            },
        );
        let entry = world.propose(local(9, 2, 21, 30)).await;
        assert_eq!(
            desk.post_card(&card(vec![entry], Vec::new())).await,
            PostResult::Posted,
            "it may be visible, so its tip stays spent"
        );
        assert!(world.alerts.alerts().iter().any(|alert| matches!(
            alert,
            AdminAlert::JournalFailure {
                attempt: Some(_),
                ..
            }
        )));
    }
}
