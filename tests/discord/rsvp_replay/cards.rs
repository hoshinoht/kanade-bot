use kanade::bot::transport::{DiscordTransport, Op, Step};
use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{RsvpState, RunStatus};
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn rows_6_7_conflicts_never_guess_a_member_answer() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    world
        .fake
        .seed_reactions(message, "❌", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.conflicts, 1);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn rows_11_to_13_card_failures_and_two_page_cap_skip_without_guessing() {
    for step in [
        Some(Step::Reject(
            kanade::bot::transport::RejectionKind::MissingAccess,
        )),
        Some(Step::Ambiguous {
            kind: kanade::bot::transport::AmbiguousKind::Timeout,
            applied: false,
        }),
        None,
    ] {
        let world = World::new();
        let (run, message) = world.carded_run().await;
        if let Some(step) = step {
            world.fake.script(Op::ReactionUsers, step);
        } else {
            world.fake.seed_reactions(
                message,
                "✅",
                ReactionType::Normal,
                (1..=201).map(Id::new).collect(),
            );
        }
        let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
        assert_eq!(report.applied, 0);
        assert!(world.snapshot(&run).await.rsvps.is_empty());
    }
}

#[tokio::test]
async fn deleted_old_card_does_not_create_an_answer() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world.fake.delete_message(Id::new(CHANNEL), message).await;
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_15_grouped_card_reads_once_and_applies_each_run() {
    let world = World::new();
    let first = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            kanade::domain::schedule::RunStatus::Planned,
        )
        .await;
    let second = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(25),
            kanade::domain::schedule::RunStatus::Planned,
        )
        .await;
    world.card(&first, 400).await;
    world.store.test_map_card_run("400", &second);
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!((report.messages, report.applied), (1, 2));
    assert_eq!(
        world.fake.count(Op::ReactionUsers),
        4,
        "cached grouped snapshot"
    );
    assert_eq!(
        world.snapshot(&first).await.rsvps[0].state,
        kanade::domain::schedule::RsvpState::Yes
    );
    assert_eq!(
        world.snapshot(&second).await.rsvps[0].state,
        kanade::domain::schedule::RsvpState::Yes
    );
}

#[tokio::test]
async fn rows_21_21b_21c_moved_old_cards_are_guard_only_not_add_evidence() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now() + chrono::TimeDelta::minutes(1));
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(world.clock.get()),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, now() + chrono::TimeDelta::hours(30), &policy())
    .await
    .unwrap();
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
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(SELF)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(SELF)],
    );
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        0
    );
    assert_eq!(world.snapshot(&run).await.rsvps[0].state, RsvpState::Yes);
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now() + chrono::TimeDelta::minutes(1));
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(world.clock.get()),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, now() + chrono::TimeDelta::hours(30), &policy())
    .await
    .unwrap();
    world.card(&run, 401).await;
    world
        .fake
        .delete_message(Id::new(CHANNEL), Id::new(400))
        .await;
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        1
    );
}

#[tokio::test]
async fn row_22b_retired_but_present_card_blocks_a_guarded_clear() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    let (_lease, attempt) = world.card_attempt(&run, 400).await;
    world.store.test_retire_bound_attempt(&attempt);
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
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER), Id::new(SELF)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(SELF)],
    );
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
