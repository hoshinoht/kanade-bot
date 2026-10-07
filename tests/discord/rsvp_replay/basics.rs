use kanade::domain::history::{Actor, ChangeHistory, Surface};
use kanade::domain::schedule::{RsvpSource, RsvpState};
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
async fn rows_2_4_5_switch_and_no_ops_are_idempotent() {
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
    assert_eq!(
        replay.replay(Id::new(SELF), || true, &mut ()).await.applied,
        0
    );
}
