use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use kanade::bot::cards::ReplayLive;
use kanade::bot::transport::Op;
use kanade::domain::history::{Actor, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::schedule::RunStatus;
use kanade::domain::scheduler::SchedulerService;
use twilight_model::channel::message::ReactionType;
use twilight_model::id::Id;

use super::support::*;

#[tokio::test]
async fn row_24_generation_cancellation_between_calls_aborts_without_writing() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let current = Arc::new(AtomicBool::new(true));
    let hold = world.fake.hold(Op::ReactionUsers);
    let mut replay = world.replay();
    let mut live = ();
    let (report, ()) = tokio::join!(
        replay.replay(Id::new(SELF), || current.load(Ordering::SeqCst), &mut live),
        async {
            hold.entered().await;
            current.store(false, Ordering::SeqCst);
            hold.release();
        }
    );
    assert!(report.aborted);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn current_clock_is_checked_after_live_drain_before_commit() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    world.clock.set(now() + chrono::TimeDelta::days(2));
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut ())
            .await
            .applied,
        0
    );
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_9c_status_changed_during_http_is_stale_not_reblessed() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let hold = world.fake.hold(Op::ReactionUsers);
    let mut replay = world.replay();
    let mut live = ();
    let (report, ()) = tokio::join!(replay.replay(Id::new(SELF), || true, &mut live), async {
        hold.entered().await;
        SchedulerService::new(
            world.store.clone(),
            RandomIds,
            kanade::bot::delivery::FixedClock(now()),
        )
        .as_origin(Origin::new(Actor::admin("race"), Surface::AdminPortal))
        .set_run_status(&run, RunStatus::Cancelled)
        .await
        .unwrap();
        hold.release();
    });
    assert_eq!(report.applied, 0);
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

struct AdvanceClockLive {
    clock: TestClock,
    calls: usize,
}
impl ReplayLive for AdvanceClockLive {
    async fn drain(&mut self) -> BTreeSet<String> {
        self.calls += 1;
        if self.calls == 3 {
            self.clock.set(now() + chrono::TimeDelta::days(2));
        }
        BTreeSet::new()
    }
}

struct TouchOnce {
    message: String,
    touched: bool,
}

impl ReplayLive for TouchOnce {
    async fn drain(&mut self) -> BTreeSet<String> {
        if self.touched {
            BTreeSet::new()
        } else {
            self.touched = true;
            BTreeSet::from([self.message.clone()])
        }
    }
}

struct TouchAlways {
    message: String,
}

impl ReplayLive for TouchAlways {
    async fn drain(&mut self) -> BTreeSet<String> {
        BTreeSet::from([self.message.clone()])
    }
}

#[tokio::test]
async fn row_9c_clock_advances_after_drain_and_before_commit() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let mut live = AdvanceClockLive {
        clock: world.clock.clone(),
        calls: 0,
    };
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut live)
            .await
            .applied,
        0
    );
    assert!(world.snapshot(&run).await.rsvps.is_empty());
}

#[tokio::test]
async fn row_10_live_event_restarts_once_and_retry_cap_is_three_restarts() {
    let world = World::new();
    let (run, message) = world.carded_run().await;
    world
        .fake
        .seed_reactions(message, "✅", ReactionType::Normal, vec![Id::new(MEMBER)]);
    let mut live = TouchOnce {
        message: message.to_string(),
        touched: false,
    };
    assert_eq!(
        world
            .replay()
            .replay(Id::new(SELF), || true, &mut live)
            .await
            .applied,
        1
    );
    assert_eq!(world.fake.count(Op::ReactionUsers), 4);
    assert_eq!(
        world.snapshot(&run).await.rsvps[0].state,
        kanade::domain::schedule::RsvpState::Yes
    );
    let world = World::new();
    let (_run, message) = world.carded_run().await;
    let mut live = TouchAlways {
        message: message.to_string(),
    };
    let report = world
        .replay()
        .replay(Id::new(SELF), || true, &mut live)
        .await;
    assert_eq!(report.applied, 0);
    assert_eq!(world.fake.count(Op::ReactionUsers), 0);
}
