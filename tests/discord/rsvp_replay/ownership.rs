use kanade::bot::transport::DiscordTransport;
use kanade::domain::history::{Actor, ChangeHistory, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{RsvpSource, RsvpState};
use kanade::domain::scheduler::SchedulerService;
use kanade::extract::pipeline::{ChatAnswer, Outbox};
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_3_guard_g_c_requires_the_bot_reaction_for_removal() {
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
async fn row_8a_portal_answer_after_reaction_wins_over_stale_card() {
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
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(Actor::admin("portal"), Surface::AdminPortal))
    .portal_answer(&run, &MEMBER.to_string(), Some(RsvpState::No))
    .await
    .unwrap();
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::No);
}

#[tokio::test]
async fn row_8b_slash_rsvp_cannot_be_removed_by_card_absence() {
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
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(
        Origin::new(Actor::member(MEMBER.to_string()), Surface::Discord)
            .with_request_id("discord:123"),
    )
    .set_rsvp(&run, &MEMBER.to_string(), RsvpState::No, RsvpSource::Chat)
    .await
    .unwrap();
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(SELF)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.state, RsvpState::No);
    assert_eq!(answer.source, RsvpSource::Chat);
}

#[tokio::test]
async fn row_8c_card_outbox_both_writes_succeed_and_source_chat_wins() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    let head = world.store.history_head().await.unwrap().seq;
    let outbox = world.card_outbox();
    Outbox::answers(
        &outbox,
        vec![ChatAnswer {
            channel_id: CHANNEL.to_string(),
            run_id: run.clone(),
            user_ids: vec![MEMBER.to_string()],
            state: RsvpState::Yes,
        }],
    )
    .await;
    assert_eq!(world.store.history_head().await.unwrap().seq, head + 2);
    let first = world.store.load_change(head + 1).await.unwrap().unwrap();
    assert!(
        first
            .origin
            .request_id
            .unwrap()
            .starts_with("extract-answer:")
    );
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.source, RsvpSource::Chat);
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_8d_card_outbox_second_write_failure_keeps_extract_request_local() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    let head = world.store.history_head().await.unwrap().seq;
    world.store.fail_commit_after(1);
    let outbox = world.card_outbox();
    Outbox::answers(
        &outbox,
        vec![ChatAnswer {
            channel_id: CHANNEL.to_string(),
            run_id: run.clone(),
            user_ids: vec![MEMBER.to_string()],
            state: RsvpState::Yes,
        }],
    )
    .await;
    assert_eq!(world.store.history_head().await.unwrap().seq, head + 1);
    let first = world.store.load_change(head + 1).await.unwrap().unwrap();
    assert!(
        first
            .origin
            .request_id
            .unwrap()
            .starts_with("extract-answer:")
    );
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.source, RsvpSource::Reaction);
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn guard_g_a_nonreaction_answer_is_not_removed_when_remote_is_absent() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(Actor::admin("portal"), Surface::AdminPortal))
    .portal_answer(&run, &MEMBER.to_string(), Some(RsvpState::No))
    .await
    .unwrap();
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(SELF)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.state, RsvpState::No);
    assert_eq!(answer.source, RsvpSource::Chat);
}

#[tokio::test]
async fn guard_g_b_unreadable_mapped_card_prevents_removal() {
    let world = World::new();
    let (run, fresh) = world.carded_run().await;
    world.card(&run, 401).await;
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
        .seed_reactions(fresh, "✅", ReactionType::Normal, vec![Id::new(SELF)]);
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(401))
        .await;
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_22_cleared_card_mapping_is_still_read_for_removal_safety() {
    use kanade::bot::delivery::DebugCardStore;

    let world = World::new();
    let (run, fresh) = world.carded_run().await;
    world.card(&run, 401).await;
    world.store.clear_debug_card("401", now()).await.unwrap();
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
        .seed_reactions(fresh, "✅", ReactionType::Normal, vec![Id::new(SELF)]);
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(401))
        .await;
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_22_unbound_reminder_mapping_is_still_guarded() {
    let world = World::new();
    let (run, fresh) = world.carded_run().await;
    world.unbound_card_mapping(&run, 401).await;
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
        .seed_reactions(fresh, "✅", ReactionType::Normal, vec![Id::new(SELF)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}

#[tokio::test]
async fn row_14_moderator_remove_all_without_bot_reaction_cannot_clear() {
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
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(SELF)]);
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
}
