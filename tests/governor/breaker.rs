use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::TimeDelta;
use kanade::infrastructure::llm::governor::{
    BreakerState, CallKind, Governor, Outcome, Permit, Priority, Refused, Role,
};
use tokio::time::Instant;

use crate::support::{LONG, build, settle, single, snap, ticket, wall};

/// Rate ceiling far above anything these tests send.
fn fast(permits: u32) -> Arc<Governor> {
    let mut config = single(permits, 6_000);
    config.groups[0].burst = Some(1_000);
    build(&config)
}

fn hold(governor: &Governor) -> Permit {
    governor
        .try_acquire(Role::Chat, CallKind::Chat, "holder")
        .expect("free permit")
}

async fn send(permit: &Permit, outcome: Outcome) {
    permit.begin_request(LONG).await.unwrap().finish(outcome);
}

/// Default policy: 30 s cooldown jittered by 0.5 × 50 % → 37.5 s.
const FIRST_OPEN: Duration = Duration::from_millis(37_500);

#[tokio::test(start_paused = true)]
async fn consecutive_failures_open_the_breaker_and_shed_fast() {
    let governor = fast(2);
    let permit = hold(&governor);
    for _ in 0..4 {
        send(&permit, Outcome::Timeout).await;
    }
    assert_eq!(snap(&governor).breaker.failures, 4);
    send(&permit, Outcome::Rejected).await;
    assert_eq!(snap(&governor).breaker.failures, 0);
    for _ in 0..5 {
        send(&permit, Outcome::TransientFailure).await;
    }
    let breaker = snap(&governor).breaker;
    assert_eq!(breaker.state, BreakerState::Open);
    assert_eq!(breaker.failures, 5);
    assert_eq!(breaker.since, wall());
    assert_eq!(
        breaker.retry_at,
        Some(wall() + TimeDelta::milliseconds(37_500))
    );

    let probe_at = Instant::now() + FIRST_OPEN;
    let refused = Refused::Unavailable {
        retry_at: Some(probe_at),
    };
    assert_eq!(permit.begin_request(LONG).await.unwrap_err(), refused);
    assert_eq!(
        governor
            .acquire(Role::Chat, ticket(Priority::Admin, "admin"), LONG)
            .await
            .unwrap_err(),
        refused
    );
    assert_eq!(
        governor
            .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
            .unwrap_err(),
        refused
    );
    assert_eq!(snap(&governor).counters.shed_unavailable, 3);
}

#[tokio::test(start_paused = true)]
async fn backend_unavailable_opens_at_once() {
    let governor = fast(1);
    let permit = hold(&governor);
    send(&permit, Outcome::BackendUnavailable).await;
    assert_eq!(snap(&governor).breaker.state, BreakerState::Open);
}

#[tokio::test(start_paused = true)]
async fn admission_refusals_say_nothing_about_backend_health() {
    let governor = fast(1);
    let permit = hold(&governor);
    for _ in 0..20 {
        send(&permit, Outcome::AdmissionRefused).await;
    }
    let snapshot = snap(&governor);
    assert_eq!(snapshot.breaker.state, BreakerState::Closed);
    assert_eq!(snapshot.counters.admission_refused, 20);
}

#[tokio::test(start_paused = true)]
async fn stragglers_from_before_the_trip_do_not_close_it() {
    let governor = fast(2);
    let a = hold(&governor);
    let b = hold(&governor);
    let late = b.begin_request(LONG).await.unwrap();
    send(&a, Outcome::BackendUnavailable).await;
    late.finish(Outcome::Success);
    assert_eq!(snap(&governor).breaker.state, BreakerState::Open);
}

#[tokio::test(start_paused = true)]
async fn half_open_admits_exactly_one_probe() {
    let governor = fast(2);
    let a = hold(&governor);
    let b = hold(&governor);
    send(&a, Outcome::BackendUnavailable).await;
    tokio::time::advance(FIRST_OPEN - Duration::from_millis(1)).await;
    assert_eq!(snap(&governor).breaker.state, BreakerState::Open);
    tokio::time::advance(Duration::from_millis(1)).await;
    let breaker = snap(&governor).breaker;
    assert_eq!(breaker.state, BreakerState::HalfOpen);
    assert_eq!(breaker.retry_at, None);

    let probe = a.begin_request(LONG).await.unwrap();
    assert!(probe.is_probe());
    assert_eq!(
        b.begin_request(LONG).await.unwrap_err(),
        Refused::Unavailable { retry_at: None }
    );
    // An abandoned probe frees the slot for another.
    drop(probe);
    let probe = b.begin_request(LONG).await.unwrap();
    assert!(probe.is_probe());
    probe.finish(Outcome::AdmissionRefused);
    let probe = a.begin_request(LONG).await.unwrap();
    assert!(probe.is_probe());
    probe.finish(Outcome::Success);
    let breaker = snap(&governor).breaker;
    assert_eq!((breaker.state, breaker.failures), (BreakerState::Closed, 0));
    assert!(!a.begin_request(LONG).await.unwrap().is_probe());
}

#[tokio::test(start_paused = true)]
async fn a_failed_probe_reopens_with_a_longer_cooldown() {
    let governor = fast(1);
    let permit = hold(&governor);
    send(&permit, Outcome::BackendUnavailable).await;
    tokio::time::advance(FIRST_OPEN).await;
    let probe = permit.begin_request(LONG).await.unwrap();
    assert!(probe.is_probe());
    probe.finish(Outcome::Timeout);
    let breaker = snap(&governor).breaker;
    assert_eq!(breaker.state, BreakerState::Open);
    // 60 s doubled cooldown, jittered by 25 %.
    assert_eq!(breaker.retry_at, Some(wall() + TimeDelta::seconds(75)));
}

#[tokio::test(start_paused = true)]
async fn queued_work_waits_out_the_outage_and_drains_gradually() {
    let governor = fast(4);
    let failing = hold(&governor);
    let holders: Arc<Mutex<Vec<Permit>>> = Arc::default();
    let tasks: Vec<_> = (0..6)
        .map(|i| {
            let governor = governor.clone();
            let holders = holders.clone();
            tokio::spawn(async move {
                let permit = governor
                    .acquire(
                        Role::Extraction,
                        ticket(Priority::Extraction, &format!("run {i}")),
                        LONG,
                    )
                    .await
                    .unwrap();
                holders.lock().unwrap().push(permit);
            })
        })
        .collect();
    settle().await;
    // Three were granted at once alongside the failing holder; three queue.
    assert_eq!(holders.lock().unwrap().len(), 3);
    send(&failing, Outcome::BackendUnavailable).await;
    drop(failing);
    holders.lock().unwrap().clear();
    settle().await;
    assert_eq!(snap(&governor).permits.in_use, 0);
    assert_eq!(snap(&governor).queue.len(), 3);

    tokio::time::advance(FIRST_OPEN).await;
    settle().await;
    let probe_holder = holders.lock().unwrap().pop().expect("one permit half-open");
    assert_eq!(snap(&governor).permits.in_use, 1);
    let probe = probe_holder.begin_request(LONG).await.unwrap();
    assert!(probe.is_probe());
    probe.finish(Outcome::Success);
    drop(probe_holder);
    settle().await;
    // Drain starts at one permit and adds one per 5 s step.
    assert_eq!(snap(&governor).permits.in_use, 1);
    tokio::time::advance(Duration::from_secs(5)).await;
    settle().await;
    assert_eq!(snap(&governor).permits.in_use, 2);
    tokio::time::advance(Duration::from_secs(5)).await;
    settle().await;
    assert_eq!(snap(&governor).permits.in_use, 2);
    assert!(snap(&governor).queue.is_empty());
    for task in tasks {
        task.await.unwrap();
    }
}
