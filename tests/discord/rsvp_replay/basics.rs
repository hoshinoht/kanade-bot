use kanade::domain::history::{Actor, ChangeHistory, Surface};
use kanade::domain::schedule::{RsvpSource, RsvpState};

use super::support::*;

#[tokio::test]
async fn row_1_add_is_attributed_to_member_discord_with_replay_request() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    world.react(message.get(), "✅", &[MEMBER]);
    assert_eq!(world.replay_once().await.applied, 1);
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

/// A replay-written answer stays reaction-owned: each later pass switches
/// it to the card's current reaction.
#[tokio::test]
async fn row_2_replay_switches_yes_to_no_and_no_to_yes() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    let mut replay = world.replay();
    for (state, emoji, other) in [
        (RsvpState::No, "❌", "✅"),
        (RsvpState::Yes, "✅", "❌"),
        (RsvpState::No, "❌", "✅"),
    ] {
        world.react(message.get(), other, &[]);
        world.react(message.get(), emoji, &[MEMBER]);
        assert_eq!(applied(&mut replay).await, 1, "{state:?}");
        assert_eq!(world.snapshot(&run).await.rsvps[0].state, state);
    }
}

#[tokio::test]
async fn row_4_add_then_remove_leaves_no_answer_to_replay() {
    let world = World::new().await;
    let (run, _) = world.carded_run().await;
    let head = world.store.history_head().await.unwrap().seq;
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.store.history_head().await.unwrap().seq, head);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_5_remove_then_readd_same_emoji_is_a_history_noop() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    world.member_reacts_yes(&run).await;
    world.react(message.get(), "✅", &[MEMBER]);
    let head = world.store.history_head().await.unwrap().seq;
    assert_eq!(world.replay_once().await.applied, 0);
    assert_eq!(world.store.history_head().await.unwrap().seq, head);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}
