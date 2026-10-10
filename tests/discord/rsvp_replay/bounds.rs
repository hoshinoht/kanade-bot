use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::RunStatus;
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn rows_19_20_23_terminal_otot_started_and_past_runs_are_not_candidates() {
    for (status, offset) in [
        (RunStatus::Done, chrono::TimeDelta::days(1)),
        (RunStatus::Cancelled, chrono::TimeDelta::days(1)),
        (RunStatus::Otot, chrono::TimeDelta::days(1)),
        (RunStatus::Planned, chrono::TimeDelta::seconds(-1)),
    ] {
        let world = World::new();
        let run = world
            .run(&[MEMBER], world.clock.get() + offset, status)
            .await;
        world.card(&run, 400).await;
        world.fake.seed_reactions(
            Id::new(400),
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
            0
        );
    }
}

#[tokio::test]
async fn max_age_does_not_make_an_old_card_evidence() {
    let world = World::new();
    world.clock.set(now() - chrono::TimeDelta::days(8));
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::days(1),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now());
    world.fake.seed_reactions(
        Id::new(400),
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
        0
    );
}

#[tokio::test]
async fn row_25_old_mapped_card_is_still_read_for_guarded_removal() {
    let world = World::new();
    world.clock.set(now() - chrono::TimeDelta::days(8));
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::days(1),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now());
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
    for message in [400, 401] {
        world.fake.seed_reactions(
            Id::new(message),
            "✅",
            ReactionType::Normal,
            vec![Id::new(SELF)],
        );
    }
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!((report.messages, report.applied), (2, 1));
    assert!(world.snapshot(&run).await.rsvps.is_empty());
    assert_eq!(
        world.fake.count(kanade::bot::transport::Op::ReactionUsers),
        8
    );
}

#[tokio::test]
async fn row_23_previous_week_run_is_not_replayed_after_rollover() {
    let world = World::new();
    world.clock.set(now() - chrono::TimeDelta::days(8));
    let run = world
        .run(
            &[MEMBER],
            now() - chrono::TimeDelta::days(1),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.clock.set(now());
    world.card(&run, 401).await;
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    world.fake.seed_reactions(
        Id::new(401),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.applied, 0);
    assert_eq!(report.messages, 0, "a prior-week run is no longer eligible");
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_26_card_and_message_budgets_defer_whole_runs() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::days(1),
            RunStatus::Planned,
        )
        .await;
    for message in 400..409 {
        world.card(&run, message).await;
    }
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(
        report.messages, 0,
        "over-eight-card run is never partly planned"
    );
    let world = World::new();
    for message in 400..441 {
        let run = world
            .run(
                &[MEMBER],
                now() + chrono::TimeDelta::days(1),
                RunStatus::Planned,
            )
            .await;
        world.card(&run, message).await;
    }
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.messages, 40);
    assert!(report.skipped >= 1, "41st run is deferred whole");
}

#[tokio::test]
async fn row_26_per_run_eight_card_cap_skips_whole_run_then_continues_shared_card() {
    let world = World::new();
    let first = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    let oversized = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(25),
            RunStatus::Planned,
        )
        .await;
    let later = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(26),
            RunStatus::Planned,
        )
        .await;
    world.card(&first, 400).await;
    world.store.test_map_card_run("400", &oversized);
    world.store.test_map_card_run("400", &later);
    for message in 401..409 {
        world.card(&oversized, message).await;
    }
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!((report.messages, report.applied), (1, 2));
    assert!(world.snapshot(&oversized).await.rsvps.is_empty());
    assert_eq!(world.snapshot(&first).await.rsvps.len(), 1);
    assert_eq!(world.snapshot(&later).await.rsvps.len(), 1);
}

#[tokio::test]
async fn row_26_pass_budget_counts_shared_grouped_cards_and_skips_whole_runs() {
    let world = World::new();
    let mut runs = Vec::new();
    for hours in 24..=32 {
        runs.push(
            world
                .run(
                    &[MEMBER],
                    now() + chrono::TimeDelta::hours(hours),
                    RunStatus::Planned,
                )
                .await,
        );
    }
    for message in 400..408 {
        world.card(&runs[0], message).await;
    }
    world.store.test_map_card_run("407", &runs[1]);
    for message in 408..415 {
        world.card(&runs[1], message).await;
    }
    world.store.test_map_card_run("414", &runs[2]);
    for message in 415..421 {
        world.card(&runs[2], message).await;
    }
    world.store.test_map_card_run("420", &runs[3]);
    for message in 421..427 {
        world.card(&runs[3], message).await;
    }
    world.store.test_map_card_run("426", &runs[4]);
    for message in 427..433 {
        world.card(&runs[4], message).await;
    }
    world.store.test_map_card_run("432", &runs[5]);
    for message in 433..439 {
        world.card(&runs[5], message).await;
    }
    world.store.test_map_card_run("438", &runs[6]);
    world.card(&runs[6], 439).await;
    world.card(&runs[7], 440).await;
    world.card(&runs[7], 441).await;
    world.store.test_map_card_run("400", &runs[8]);
    for message in [400, 408, 415, 421, 427, 433, 439] {
        world.fake.seed_reactions(
            Id::new(message),
            "✅",
            ReactionType::Normal,
            vec![Id::new(MEMBER)],
        );
    }
    let report = world.replay().replay(Id::new(SELF), || true, &mut ()).await;
    assert_eq!(report.messages, 40, "the shared card is counted once");
    assert_eq!(
        report.applied, 8,
        "the too-large run is deferred, then planning continues"
    );
    assert!(world.snapshot(&runs[7]).await.rsvps.is_empty());
    for run in [
        &runs[0], &runs[1], &runs[2], &runs[3], &runs[4], &runs[5], &runs[6], &runs[8],
    ] {
        assert_eq!(world.snapshot(run).await.rsvps.len(), 1);
    }
}
