use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{RsvpSource, RsvpState};
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_3_and_g_own_reaction_guard_control_removal() {
    for own in [false, true] {
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
        if own {
            world
                .fake
                .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(SELF)]);
        }
        world.replay().replay(Id::new(SELF), || true, &mut ()).await;
        assert_eq!(
            world.snapshot(&run).await.rsvps.is_empty(),
            own,
            "own={own}"
        );
    }
}

#[tokio::test]
async fn p_portal_slash_and_chat_answers_are_local_wins() {
    for (surface, request) in [
        (Surface::AdminPortal, None),
        (Surface::Discord, Some("discord:123")),
        (Surface::Discord, Some("extract-answer:x")),
    ] {
        let world = World::new();
        let (run, message) = world.carded_run().await;
        let origin = request.map_or_else(
            || Origin::new(Actor::admin("portal"), surface),
            |id| Origin::new(Actor::member(MEMBER.to_string()), surface).with_request_id(id),
        );
        SchedulerService::new(
            world.store.clone(),
            RandomIds,
            kanade::bot::delivery::FixedClock(now()),
        )
        .as_origin(origin)
        .set_rsvp(&run, &MEMBER.to_string(), RsvpState::Yes, RsvpSource::Chat)
        .await
        .unwrap();
        world
            .fake
            .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
        assert_eq!(
            world
                .replay()
                .replay(Id::new(SELF), || true, &mut ())
                .await
                .applied,
            0
        );
        assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
    }
}

#[tokio::test]
async fn p_extract_first_write_request_id_stays_local_when_second_write_failed() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(
        Origin::new(Actor::member(MEMBER.to_string()), Surface::Discord)
            .with_request_id("extract-answer:synthetic"),
    )
    .apply_reaction(&run, &MEMBER.to_string(), "✅", true)
    .await
    .unwrap();
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        0
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
    assert_eq!(
        world.snapshot(&run).await.rsvps[0].source,
        RsvpSource::Reaction
    );
}
