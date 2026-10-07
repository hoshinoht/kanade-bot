use std::sync::{Arc, Mutex};

use kanade::domain::history::{Actor, ChangeHistory, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::DeclineNoticeStore;
use kanade::domain::schedule::RsvpState;
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_8e_portal_clear_stays_cleared_and_replay_is_idempotent() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    let mut service = SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    );
    service
        .as_origin(Origin::new(
            Actor::member(MEMBER.to_string()),
            Surface::Discord,
        ))
        .apply_reaction(&run, &MEMBER.to_string(), "✅", true)
        .await
        .unwrap();
    service
        .as_origin(Origin::new(Actor::admin("portal"), Surface::AdminPortal))
        .portal_answer(&run, &MEMBER.to_string(), None)
        .await
        .unwrap();
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let mut replay = world.replay();
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        0
    );
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        0
    );
    assert!(world.snapshot(&run).await.rsvps.is_empty());
    let _ = world.store.history_head().await.unwrap();
}

#[tokio::test]
async fn replay_written_answer_remains_reaction_owned_on_the_next_pass() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let mut replay = world.replay();
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        1
    );
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![]);
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        1
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::No);
}

#[tokio::test]
async fn row_13_replay_queues_one_refresh_and_never_creates_decline_notice() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    let observed = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&observed);
    assert!(world.store.observe_run_writes(Arc::new(move |runs| {
        capture.lock().unwrap().extend_from_slice(runs)
    })));
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        1
    );
    assert_eq!(
        observed.lock().unwrap().as_slice(),
        std::slice::from_ref(&run)
    );
    assert!(
        world
            .store
            .decline_notice(&run, &MEMBER.to_string())
            .await
            .unwrap()
            .is_none()
    );
}
