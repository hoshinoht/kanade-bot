use super::*;
use kanade::bot::cards::OFFLINE_CONFLICT_NOTICE;
use kanade::bot::events::{CardIndex, LookupError, ReactionRouter, RsvpReaction};
use kanade::bot::handler::Reactions;
use kanade::chat::driver::{FollowUpRequest, RejectionFollowUp};
use std::{future::Future, pin::Pin, sync::Mutex};
use tokio::sync::mpsc;
use twilight_model::channel::message::ReactionType;

const SELF: u64 = 1003; // An admin: ignoring self must precede authority checks.

struct NoIndex;
impl CardIndex for NoIndex {
    async fn runs_for_message(&self, _: MessageId) -> Result<Vec<String>, LookupError> {
        Ok(Vec::new())
    }
}

#[derive(Default)]
struct FollowUps(Mutex<Vec<FollowUpRequest>>);
impl RejectionFollowUp for FollowUps {
    fn rejected(&self, request: FollowUpRequest) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(request);
        })
    }
}

type Worker = Reactions<
    MemoryScheduleStore,
    FakeDiscord,
    Ids,
    AlertRecorder,
    NoIndex,
    SchedulerService<Arc<MemoryScheduleStore>, Ids, FixedClock>,
>;

fn worker(world: &World, follow_up: &Arc<FollowUps>) -> Worker {
    Reactions {
        desk: Arc::new(desk(
            &world.store,
            &world.discord,
            &world.alerts,
            &world.ids,
        )),
        rsvp: ReactionRouter::new(
            NoIndex,
            SchedulerService::new(
                Arc::clone(&world.store),
                world.ids.clone(),
                FixedClock(now()),
            ),
        ),
        follow_up: Some(follow_up.clone()),
        decline_retraction: None,
        clock: Arc::new(now),
    }
}

fn live_reaction(message: MessageId, user: &str, added: bool) -> RsvpReaction {
    RsvpReaction {
        channel_id: Id::new(CHANNEL.parse().unwrap()),
        message_id: message,
        user_id: Id::new(user.parse().unwrap()),
        answer: RsvpAnswer::Yes,
        added,
        display_name: "Synthetic reactor".into(),
    }
}

async fn pending(world: &World, source: ProposalSource) -> (String, MessageId) {
    let entry = world.propose_from(local(9, 2, 21, 30), source).await;
    let id = entry.proposal_id.clone();
    world.desk.post_card(&card(vec![entry], Vec::new())).await;
    let message = Id::new(world.message_of(&id).await.unwrap().parse().unwrap());
    (id, message)
}

fn seed(world: &World, message: MessageId, emoji: &str, users: &[&str]) {
    world.discord.seed_reactions(
        message,
        emoji,
        ReactionType::Normal,
        users
            .iter()
            .map(|id| Id::new(id.parse().unwrap()))
            .collect(),
    );
}

#[tokio::test]
async fn offline_no_rejects_extraction_and_chat_without_stale_follow_up() {
    for source in [ProposalSource::Extraction, ProposalSource::Chat] {
        let world = World::new().await;
        let (id, message) = pending(&world, source).await;
        seed(&world, message, "❌", &[MY]);
        let restarted = desk(&world.store, &world.discord, &world.alerts, &world.ids);
        let report = restarted.replay_reactions(Id::new(SELF), || true).await;
        assert_eq!(report.rejected, 1);
        assert_eq!(world.status(&id).await, DraftStatus::Rejected);
        assert!(world.last_edit_content().ends_with("❌ rejected by Mylene"));
        assert_eq!(world.creates().len(), 1, "no stale chat follow-up");
    }
}

#[tokio::test]
async fn offline_yes_approves_extraction_and_chat_through_scheduler() {
    for source in [ProposalSource::Extraction, ProposalSource::Chat] {
        let world = World::new().await;
        let (id, message) = pending(&world, source).await;
        seed(&world, message, "✅", &[MY]);
        let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
        assert_eq!(report.approved, 1);
        assert_eq!(world.status(&id).await, DraftStatus::Merged);
        assert!(world.last_edit_content().ends_with("✅ applied by Mylene"));
        assert_eq!(
            world
                .store
                .load(&Scope::Run(world.run.clone()))
                .await
                .unwrap()
                .runs[0]
                .datetime,
            local(9, 2, 21, 30)
        );
        let repeated = world.desk.replay_reactions(Id::new(SELF), || true).await;
        assert_eq!(repeated.messages, 0, "closed cards are not scanned");
    }
}

#[tokio::test]
async fn offline_unauthorised_only_and_bot_seeds_leave_pending_silently() {
    for source in [ProposalSource::Extraction, ProposalSource::Chat] {
        let world = World::new().await;
        let (id, message) = pending(&world, source).await;
        for emoji in ["✅", "❌"] {
            seed(&world, message, emoji, &[STRANGER, ADMIN]);
        }
        let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
        assert_eq!(
            (report.approved, report.rejected, report.conflicts),
            (0, 0, 0)
        );
        assert_eq!(world.status(&id).await, DraftStatus::Submitted);
        assert_eq!(world.discord.count(Op::Edit), 0);
        assert_eq!(world.creates().len(), 1);
    }
}

#[tokio::test]
async fn offline_opposing_authorised_answers_keep_pending_with_persistent_refresh_note() {
    for source in [ProposalSource::Extraction, ProposalSource::Chat] {
        let world = World::new().await;
        let (id, message) = pending(&world, source).await;
        seed(&world, message, "✅", &[MY]);
        seed(&world, message, "❌", &[ALVIN]);
        let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
        assert_eq!(report.conflicts, 1);
        assert_eq!(world.status(&id).await, DraftStatus::Submitted);
        assert!(world.last_edit_content().ends_with(OFFLINE_CONFLICT_NOTICE));
        world.desk.refresh(&message.to_string()).await;
        assert!(
            world.last_edit_content().ends_with(OFFLINE_CONFLICT_NOTICE),
            "ordinary refresh preserves it"
        );
        let Some(Call::Edit { edit, .. }) = world.discord.calls().pop() else {
            panic!("edit");
        };
        assert!(edit.allowed_mentions.users.is_empty());
        assert_eq!(
            world
                .desk
                .on_reaction(&message.to_string(), MY, RsvpAnswer::No, false)
                .await,
            CardReaction::Ignored
        );
        world
            .desk
            .on_reaction(&message.to_string(), MY, RsvpAnswer::No, true)
            .await;
        assert_eq!(world.status(&id).await, DraftStatus::Rejected);
        assert!(!world.last_edit_content().contains(OFFLINE_CONFLICT_NOTICE));
    }
}

#[tokio::test]
async fn offline_unauthorised_opposite_does_not_block_an_authorised_decision() {
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Chat).await;
    seed(&world, message, "✅", &[STRANGER]);
    seed(&world, message, "❌", &[MY]);
    let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
    assert_eq!((report.rejected, report.conflicts), (1, 0));
    assert_eq!(world.status(&id).await, DraftStatus::Rejected);
}

#[tokio::test]
async fn offline_deleted_message_is_skipped_and_other_cards_still_reject() {
    let world = World::new().await;
    let (deleted, message) = pending(&world, ProposalSource::Extraction).await;
    world
        .discord
        .delete_message(Id::new(CHANNEL.parse().unwrap()), message)
        .await;
    let (live, message) = pending(&world, ProposalSource::Chat).await;
    seed(&world, message, "❌", &[MY]);
    let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
    assert_eq!((report.skipped, report.rejected), (1, 1));
    assert_eq!(world.status(&deleted).await, DraftStatus::Submitted);
    assert_eq!(world.status(&live).await, DraftStatus::Rejected);
}

#[tokio::test]
async fn offline_incomplete_opposite_read_never_approves_and_pass_continues() {
    for step in [
        Step::Reject(RejectionKind::MissingAccess),
        Step::Ambiguous {
            kind: AmbiguousKind::Timeout,
            applied: false,
        },
    ] {
        let world = World::new().await;
        let (first, message) = pending(&world, ProposalSource::Chat).await;
        seed(&world, message, "✅", &[MY]);
        let (second, message) = pending(&world, ProposalSource::Extraction).await;
        seed(&world, message, "❌", &[MY]);
        world.discord.script(Op::ReactionUsers, Step::Succeed); // ✅ normal
        world.discord.script(Op::ReactionUsers, Step::Succeed); // ✅ burst
        world.discord.script(Op::ReactionUsers, step); // ❌ unavailable
        let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
        assert_eq!(
            (report.skipped, report.rejected, report.approved),
            (1, 1, 0)
        );
        assert_eq!(world.status(&first).await, DraftStatus::Submitted);
        assert_eq!(world.status(&second).await, DraftStatus::Rejected);
    }
}

#[tokio::test]
async fn offline_reactors_page_after_100_and_super_reactions_count() {
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Extraction).await;
    let mut users: Vec<_> = (1..=100).map(Id::new).collect();
    users.push(Id::new(MY.parse().unwrap()));
    world
        .discord
        .seed_reactions(message, "❌", ReactionType::Burst, users);
    let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
    assert_eq!(report.rejected, 1);
    assert_eq!(world.status(&id).await, DraftStatus::Rejected);
    assert!(world.discord.calls().iter().any(|call| matches!(call, Call::ReactionUsers { after: Some(after), limit: 100, .. } if after.get() == 100)));
}

#[tokio::test]
async fn offline_full_page_cap_is_incomplete_and_never_decides() {
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Chat).await;
    world.discord.seed_reactions(
        message,
        "✅",
        ReactionType::Normal,
        (1..=10_000).map(Id::new).collect(),
    );
    let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
    assert_eq!(report.skipped, 1);
    assert_eq!(world.discord.count(Op::ReactionUsers), 100);
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert_eq!(world.discord.count(Op::Edit), 0);
}

#[tokio::test]
async fn superseded_generation_reports_aborted_without_deciding_the_card() {
    use std::sync::atomic::AtomicBool;
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Chat).await;
    seed(&world, message, "❌", &[MY]);
    let current = AtomicBool::new(true);
    let http = world.discord.hold(Op::ReactionUsers);
    let (report, ()) = tokio::join!(
        world
            .desk
            .replay_reactions(Id::new(SELF), || current.load(Ordering::SeqCst)),
        async {
            http.entered().await;
            current.store(false, Ordering::SeqCst);
            http.release();
        }
    );
    assert!(report.aborted);
    assert_eq!(report.rejected, 0);
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
}

#[tokio::test]
async fn grouped_replay_combines_refusals_and_preserves_stale_line_after_conflict_note_refresh() {
    let world = World::new().await;
    let first = world.propose(local(9, 2, 21, 30)).await;
    let second = world.propose(local(9, 2, 22, 30)).await;
    let ids = [first.proposal_id.clone(), second.proposal_id.clone()];
    world
        .desk
        .post_card(&card(vec![first, second], Vec::new()))
        .await;
    let message = Id::new(world.message_of(&ids[0]).await.unwrap().parse().unwrap());
    seed(&world, message, "✅", &[MY]);
    seed(&world, message, "❌", &[ALVIN]);
    assert_eq!(
        world
            .desk
            .replay_reactions(Id::new(SELF), || true)
            .await
            .conflicts,
        2
    );
    service(&world.store, &world.ids)
        .as_origin(Origin::for_tests())
        .amend_run(&world.run, local(9, 1, 22, 0), &policy())
        .await
        .unwrap();
    seed(&world, message, "❌", &[]);
    let report = world.desk.replay_reactions(Id::new(SELF), || true).await;
    assert!(!report.aborted);
    assert_eq!(report.approved, 0);
    assert_eq!(
        world.creates().len(),
        2,
        "one grouped apply-problem notice, not one per proposal"
    );
    assert!(world.last_edit_content().ends_with("⚠️ out of date"));
    assert!(!world.last_edit_content().contains(OFFLINE_CONFLICT_NOTICE));
    for id in ids {
        assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    }
}

#[tokio::test]
async fn batch_authority_keeps_reactor_order_and_lowest_authorised_attribution() {
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Chat).await;
    let mut users: Vec<_> = (1..=100).map(Id::new).collect();
    users.extend([
        Id::new(ALVIN.parse().unwrap()),
        Id::new(MY.parse().unwrap()),
    ]);
    let approvers: Vec<_> = users
        .iter()
        .map(|user| Staff.approver(&user.to_string()))
        .collect();
    let allowed = service(&world.store, &world.ids)
        .proposal_answer_authority(&id, &approvers)
        .await
        .unwrap();
    assert!(allowed[..100].iter().all(|allowed| !allowed));
    assert_eq!(&allowed[100..], &[true, true]);
    world
        .discord
        .seed_reactions(message, "❌", ReactionType::Normal, users);
    assert_eq!(
        world
            .desk
            .replay_reactions(Id::new(SELF), || true)
            .await
            .rejected,
        1
    );
    assert!(world.last_edit_content().ends_with("❌ rejected by Mylene"));
}

#[tokio::test]
async fn live_no_queued_during_http_replay_keeps_its_chat_follow_up() {
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Chat).await;
    let follow_up = Arc::new(FollowUps::default());
    let mut worker = worker(&world, &follow_up);
    let (send, mut queue) = mpsc::unbounded_channel();
    let http = world.discord.hold(Op::ReactionUsers);
    let (report, ()) = tokio::join!(worker.replay(Id::new(SELF), || true, &mut queue), async {
        http.entered().await;
        seed(&world, message, "❌", &[MY]);
        send.send(RsvpReaction {
            channel_id: Id::new(CHANNEL.parse().unwrap()),
            message_id: message,
            user_id: Id::new(MY.parse().unwrap()),
            answer: RsvpAnswer::No,
            added: true,
            display_name: "Mylene".into(),
        })
        .unwrap();
        http.release();
    });
    assert_eq!(world.status(&id).await, DraftStatus::Rejected);
    assert_eq!(report.rejected, 0, "decided live, not by replay");
    assert!(queue.is_empty());
    let follow_ups = follow_up.0.lock().unwrap();
    assert_eq!(follow_ups.len(), 1);
    assert_eq!(follow_ups[0].card_message_id, message.to_string());
    assert_eq!(follow_ups[0].reactor_id, MY);
}

#[tokio::test]
async fn non_deciding_live_events_do_not_discard_offline_rejection() {
    for (user, added) in [(STRANGER, true), (MY, false)] {
        let world = World::new().await;
        let (id, message) = pending(&world, ProposalSource::Chat).await;
        seed(&world, message, "❌", &[MY]);
        let follow_up = Arc::new(FollowUps::default());
        let mut worker = worker(&world, &follow_up);
        let (send, mut queue) = mpsc::unbounded_channel();
        let http = world.discord.hold(Op::ReactionUsers);
        let (report, ()) = tokio::join!(worker.replay(Id::new(SELF), || true, &mut queue), async {
            http.entered().await;
            if added {
                seed(&world, message, "✅", &[user]);
            }
            send.send(live_reaction(message, user, added)).unwrap();
            http.release();
        });
        assert_eq!(world.status(&id).await, DraftStatus::Rejected);
        assert_eq!((report.rejected, report.skipped), (1, 0));
        assert_eq!(
            world.discord.count(Op::ReactionUsers),
            8,
            "a fresh read after the live event"
        );
        assert!(
            follow_up.0.lock().unwrap().is_empty(),
            "offline rejection has no stale chat follow-up"
        );
    }
}

#[tokio::test]
async fn continuous_live_events_exhaust_only_the_bounded_card_retries() {
    let world = World::new().await;
    let (id, message) = pending(&world, ProposalSource::Chat).await;
    seed(&world, message, "❌", &[MY]);
    let follow_up = Arc::new(FollowUps::default());
    let mut worker = worker(&world, &follow_up);
    let (send, mut queue) = mpsc::unbounded_channel();
    // Four reads per attempt, for the initial attempt plus three retries.
    let holds: Vec<_> = (0..16)
        .map(|_| world.discord.hold(Op::ReactionUsers))
        .collect();
    let (report, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(worker.replay(Id::new(SELF), || true, &mut queue), async {
            for (index, http) in holds.iter().enumerate() {
                http.entered().await;
                if index % 4 == 0 {
                    send.send(live_reaction(message, STRANGER, true)).unwrap();
                }
                http.release();
            }
        })
    })
    .await
    .expect("bounded replay finishes");
    assert_eq!((report.rejected, report.skipped), (0, 1));
    assert_eq!(world.discord.count(Op::ReactionUsers), 16);
    assert_eq!(world.status(&id).await, DraftStatus::Submitted);
    assert!(follow_up.0.lock().unwrap().is_empty());
}
