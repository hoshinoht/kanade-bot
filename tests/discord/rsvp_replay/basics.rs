use kanade::domain::history::{Actor, ChangeHistory, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{RsvpSource, RsvpState};
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_1_add_is_attributed_to_member_discord_with_replay_request() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 1);
    let snapshot = world.snapshot(&run).await;
    assert_eq!(snapshot.rsvps[0].state, RsvpState::Yes);
    assert_eq!(snapshot.rsvps[0].source, RsvpSource::Reaction);
    let record = world
        .store
        .load_change(world.store.history_head().await.unwrap().seq)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.origin.actor, Actor::member(MEMBER.to_string()));
    assert_eq!(record.origin.surface, Surface::Discord);
    assert!(
        record
            .origin
            .request_id
            .unwrap()
            .starts_with("rsvp-replay:")
    );
}

#[tokio::test]
async fn row_2_replay_switches_yes_to_no_and_no_to_yes() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let mut replay = world.replay();
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        1
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::No);
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![]);
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        1
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_4_add_then_remove_leaves_no_answer_to_replay() {
    let world = World::new();
    let (run, _) = world.carded_run().await;
    let head = world.store.history_head().await.unwrap().seq;
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.store.history_head().await.unwrap().seq, head);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_5_remove_then_readd_same_emoji_is_a_history_noop() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(
        Actor::member(MEMBER.to_string()),
        Surface::Discord,
    ))
    .apply_reaction(&run, &MEMBER.to_string(), "✅", true)
    .await
    .unwrap();
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let head = world.store.history_head().await.unwrap().seq;
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.store.history_head().await.unwrap().seq, head);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}
