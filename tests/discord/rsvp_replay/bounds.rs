use kanade_domain::schedule::RunStatus;

use super::support::*;

#[tokio::test]
async fn rows_19_20_23_terminal_otot_started_and_past_runs_are_not_candidates() {
    for (status, offset) in [
        (RunStatus::Done, chrono::TimeDelta::days(1)),
        (RunStatus::Cancelled, chrono::TimeDelta::days(1)),
        (RunStatus::Otot, chrono::TimeDelta::days(1)),
        (RunStatus::Planned, chrono::TimeDelta::seconds(-1)),
    ] {
        let world = World::new().await;
        let run = world
            .run(&[MEMBER], world.clock.get() + offset, status)
            .await;
        world.card(&run, 400).await;
        world.react(400, "✅", &[MEMBER]);
        assert_eq!(world.replay_once().await.applied, 0);
    }
}

#[tokio::test]
async fn max_age_does_not_make_an_old_card_evidence() {
    let world = World::new().await;
    world.clock.set(now() - chrono::TimeDelta::days(8));
    let run = world.planned(&[MEMBER], 24).await;
    world.card(&run, 400).await;
    world.clock.set(now());
    world.react(400, "✅", &[MEMBER]);
    assert_eq!(world.replay_once().await.applied, 0);
}

#[tokio::test]
async fn row_25_old_mapped_card_is_still_read_for_guarded_removal() {
    let world = World::new().await;
    world.clock.set(now() - chrono::TimeDelta::days(8));
    let run = world.planned(&[MEMBER], 24).await;
    world.card(&run, 400).await;
    world.clock.set(now());
    world.card(&run, 401).await;
    world.member_reacts_yes(&run).await;
    for message in [400, 401] {
        world.react(message, "✅", &[SELF]);
    }
    let report = world.replay_once().await;
    assert_eq!((report.messages, report.applied), (2, 1));
    assert!(world.snapshot(&run).await.rsvps.is_empty());
    assert_eq!(
        world
            .fake
            .count(kanade::discord::transport::Op::ReactionUsers),
        8
    );
}

#[tokio::test]
async fn row_23_previous_week_run_is_not_replayed_after_rollover() {
    let world = World::new().await;
    world.clock.set(now() - chrono::TimeDelta::days(8));
    let run = world.planned(&[MEMBER], -24).await;
    world.card(&run, 400).await;
    world.clock.set(now());
    world.card(&run, 401).await;
    world.react(400, "✅", &[MEMBER]);
    world.react(401, "✅", &[MEMBER]);
    let report = world.replay_once().await;
    assert_eq!(report.applied, 0);
    assert_eq!(report.messages, 0, "a prior-week run is no longer eligible");
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_26_card_and_message_budgets_defer_whole_runs() {
    let world = World::new().await;
    let run = world.planned(&[MEMBER], 24).await;
    for message in 400..409 {
        world.card(&run, message).await;
    }
    assert_eq!(
        world.replay_once().await.messages,
        0,
        "over-eight-card run is never partly planned"
    );
    let world = World::new().await;
    for message in 400..441 {
        let run = world.planned(&[MEMBER], 24).await;
        world.card(&run, message).await;
    }
    let report = world.replay_once().await;
    assert_eq!(report.messages, 40);
    assert!(report.skipped >= 1, "41st run is deferred whole");
}

#[tokio::test]
async fn row_26_per_run_eight_card_cap_skips_whole_run_then_continues_shared_card() {
    let world = World::new().await;
    let first = world.planned(&[MEMBER], 24).await;
    let oversized = world.planned(&[MEMBER], 25).await;
    let later = world.planned(&[MEMBER], 26).await;
    world.card(&first, 400).await;
    world.store.test_map_card_run("400", &oversized).await;
    world.store.test_map_card_run("400", &later).await;
    for message in 401..409 {
        world.card(&oversized, message).await;
    }
    world.react(400, "✅", &[MEMBER]);
    let report = world.replay_once().await;
    assert_eq!((report.messages, report.applied), (1, 2));
    assert!(world.snapshot(&oversized).await.rsvps.is_empty());
    assert_eq!(world.snapshot(&first).await.rsvps.len(), 1);
    assert_eq!(world.snapshot(&later).await.rsvps.len(), 1);
}

#[tokio::test]
async fn row_26_pass_budget_counts_shared_grouped_cards_and_skips_whole_runs() {
    let world = World::new().await;
    let mut runs = Vec::new();
    for hours in 24..=32 {
        runs.push(world.planned(&[MEMBER], hours).await);
    }
    // Runs 0..=5 own a card range whose last card is also mapped to the next run.
    let chain = [
        (400, 407),
        (408, 414),
        (415, 420),
        (421, 426),
        (427, 432),
        (433, 438),
    ];
    for (index, (first, last)) in chain.into_iter().enumerate() {
        for message in first..=last {
            world.card(&runs[index], message).await;
        }
        world
            .store
            .test_map_card_run(&last.to_string(), &runs[index + 1])
            .await;
    }
    world.card(&runs[6], 439).await;
    world.card(&runs[7], 440).await;
    world.card(&runs[7], 441).await;
    world.store.test_map_card_run("400", &runs[8]).await;
    for message in [400, 408, 415, 421, 427, 433, 439] {
        world.react(message, "✅", &[MEMBER]);
    }
    let report = world.replay_once().await;
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
