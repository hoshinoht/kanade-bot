use kanade::bot::rsvp_replay::ReplayReport;
use kanade::discord::transport::{Hold, Op};
use kanade_domain::history::{Actor, Origin, Surface};
use kanade_domain::schedule::{RsvpSource, RsvpState, RunStatus, StatusChange};
use kanade_domain::scheduler::{ScheduleStore, Scope};
use tokio::task::JoinHandle;
use twilight_model::id::Id;

use super::support::*;

/// Starts a replay pass whose first reaction read is held, and returns once
/// the read is in flight.
async fn held_replay(world: &World) -> (Hold, JoinHandle<ReplayReport>) {
    let held_http = world.fake.hold(Op::ReactionUsers);
    let mut replay = world.replay();
    let task = tokio::spawn(async move { replay.replay(Id::new(SELF), || true, &mut ()).await });
    held_http.entered().await;
    (held_http, task)
}

/// A planned run with card 400 on which `MEMBER` reacted ✅.
async fn reacted_run(world: &World) -> String {
    let run = world.planned(&[MEMBER], 24).await;
    world.card(&run, 400).await;
    world.react(400, "✅", &[MEMBER]);
    run
}

#[tokio::test]
async fn row_9_portal_answer_while_http_is_held_is_stale_at_commit() {
    let world = World::new().await;
    let (run, message) = world.carded_run().await;
    world.react(message.get(), "✅", &[MEMBER]);
    let (held_http, task) = held_replay(&world).await;
    world.portal_answer(&run, Some(RsvpState::No)).await;
    held_http.release();
    let report = task.await.unwrap();
    assert_eq!(report.applied, 0);
    assert!(report.skipped > 0, "the stale RSVP precondition is skipped");
    let answer = &world.snapshot(&run).await.rsvps[0];
    assert_eq!(answer.state, RsvpState::No);
    assert_eq!(answer.source, RsvpSource::Chat);
}

#[tokio::test]
async fn row_9b_amend_run_while_http_is_held_is_stale_at_commit() {
    let world = World::new().await;
    let run = reacted_run(&world).await;
    let (held_http, task) = held_replay(&world).await;
    let moved_to = now() + chrono::TimeDelta::hours(30);
    world
        .service_at(now() + chrono::TimeDelta::minutes(1))
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
    let world = World::new().await;
    let run = reacted_run(&world).await;
    let (held_http, task) = held_replay(&world).await;
    world
        .service_at(now())
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
