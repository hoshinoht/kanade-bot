use kanade::bot::transport::Op;
use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::{RunStatus, StatusChange};
use kanade::domain::scheduler::{ScheduleStore, SchedulerService, Scope};
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_9_portal_answer_while_http_is_held_is_stale_at_commit() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let held_http = world.fake.hold(Op::ReactionUsers);
    let mut replay = world.replay();
    let task = tokio::spawn(async move { replay.replay(Id::new(SELF), || true, &mut ()).await });
    held_http.entered().await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(Actor::admin("portal"), Surface::AdminPortal))
    .portal_answer(
        &run,
        &MEMBER.to_string(),
        Some(kanade::domain::schedule::RsvpState::No),
    )
    .await
    .unwrap();
    held_http.release();
    let report = task.await.unwrap();
    assert_eq!(report.applied, 0);
    assert!(report.skipped > 0, "the stale RSVP precondition is skipped");
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.state, kanade::domain::schedule::RsvpState::No);
    assert_eq!(answer.source, kanade::domain::schedule::RsvpSource::Chat);
}

#[tokio::test]
async fn row_9b_amend_run_while_http_is_held_is_stale_at_commit() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let held_http = world.fake.hold(Op::ReactionUsers);
    let mut replay = world.replay();
    let task = tokio::spawn(async move { replay.replay(Id::new(SELF), || true, &mut ()).await });
    held_http.entered().await;
    let moved_to = now() + chrono::TimeDelta::hours(30);
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now() + chrono::TimeDelta::minutes(1)),
    )
    .as_origin(Origin::new(Actor::admin("move"), Surface::AdminPortal))
    .amend_run(&run, moved_to, &policy())
    .await
    .unwrap();
    held_http.release();
    let report = task.await.unwrap();
    assert_eq!(report.applied, 0);
    assert!(report.skipped > 0, "the stale slot precondition is skipped");
    let moved = world.store.load(&Scope::Run(run.clone())).await.unwrap();
    assert_eq!(moved.runs[0].datetime, moved_to);
    assert!(moved.rsvps.is_empty());
}

#[tokio::test]
async fn row_9b_status_change_while_http_is_held_is_stale_at_commit() {
    let world = World::new();
    let run = world
        .run(
            &[MEMBER],
            now() + chrono::TimeDelta::hours(24),
            RunStatus::Planned,
        )
        .await;
    world.card(&run, 400).await;
    world.fake.seed_reactions(
        Id::new(400),
        "✅",
        ReactionType::Normal,
        vec![Id::new(MEMBER)],
    );
    let held_http = world.fake.hold(Op::ReactionUsers);
    let mut replay = world.replay();
    let task = tokio::spawn(async move { replay.replay(Id::new(SELF), || true, &mut ()).await });
    held_http.entered().await;
    SchedulerService::new(
        world.store.clone(),
        RandomIds,
        kanade::bot::delivery::FixedClock(now()),
    )
    .as_origin(Origin::new(Actor::admin("cancel"), Surface::AdminPortal))
    .set_status(
        &run,
        StatusChange {
            status: RunStatus::Cancelled,
            announce: false,
            via_portal: true,
        },
        &policy().reminders,
    )
    .await
    .unwrap();
    held_http.release();
    let report = task.await.unwrap();
    assert_eq!(report.applied, 0);
    assert!(
        report.skipped > 0,
        "the stale status precondition is skipped"
    );
    let cancelled = world.store.load(&Scope::Run(run.clone())).await.unwrap();
    assert_eq!(cancelled.runs[0].status, RunStatus::Cancelled);
    assert!(cancelled.rsvps.is_empty());
}
