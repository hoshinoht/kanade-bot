use kanade::domain::schedule::RunStatus;
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
