//! A group reached only through `try_acquire` (the rewrite role) must recover
//! from a trip on its own: its holder's first request is the half-open probe.

use std::{sync::Arc, time::Duration};

use chrono::TimeDelta;
use kanade::infrastructure::llm::governor::{
    BreakerState, CallKind, Governor, GovernorConfig, GovernorPolicy, GroupSnapshot, Outcome,
    Refused, Role, RoleConfig,
};

use crate::support::{LONG, build, group, wall};

const FIRST_OPEN: Duration = Duration::from_millis(37_500);

fn split() -> Arc<Governor> {
    let mut small = group("small", 3, 6_000, &["tiny"]);
    small.burst = Some(1_000);
    let local = |alias: &str| RoleConfig {
        alias: alias.into(),
        external: false,
    };
    build(&GovernorConfig {
        groups: vec![group("local", 2, 60, &["big"]), small],
        roles: [
            (Role::Chat, local("big")),
            (Role::Extraction, local("big")),
            (Role::Rewrite, local("tiny")),
        ]
        .into_iter()
        .collect(),
        policy: GovernorPolicy::default(),
    })
}

fn small(governor: &Governor) -> GroupSnapshot {
    governor
        .snapshot(wall())
        .into_iter()
        .find(|g| g.name == "small")
        .expect("small group")
}

fn rewrite(governor: &Governor) -> Result<kanade::infrastructure::llm::governor::Permit, Refused> {
    governor.try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
}

async fn trip(governor: &Governor, outcome: Outcome, times: usize) {
    let permit = rewrite(governor).unwrap();
    for _ in 0..times {
        permit.begin_request(LONG).await.unwrap().finish(outcome);
    }
    assert_eq!(small(governor).breaker.state, BreakerState::Open);
    assert!(matches!(
        rewrite(governor).unwrap_err(),
        Refused::Unavailable { retry_at: Some(_) }
    ));
}

async fn recovers_through_a_try_acquire_probe(outcome: Outcome, times: usize) {
    let governor = split();
    trip(&governor, outcome, times).await;
    tokio::time::advance(FIRST_OPEN).await;
    assert_eq!(small(&governor).breaker.state, BreakerState::HalfOpen);

    let prober = rewrite(&governor).expect("half-open lends its single slot");
    assert_eq!(rewrite(&governor).unwrap_err(), Refused::Busy);
    let probe = prober.try_begin_request().unwrap();
    assert!(probe.is_probe());
    probe.finish(Outcome::Success);
    assert_eq!(small(&governor).breaker.state, BreakerState::Closed);

    // Drain ramp: one permit at recovery, one more per 5 s step.
    assert_eq!(rewrite(&governor).unwrap_err(), Refused::Busy);
    tokio::time::advance(Duration::from_secs(5)).await;
    let second = rewrite(&governor).expect("second step");
    assert!(!second.try_begin_request().unwrap().is_probe());
    tokio::time::advance(Duration::from_secs(5)).await;
    let _third = rewrite(&governor).expect("third step");
    assert_eq!(small(&governor).permits.in_use, 3);
    // The chat group was never involved.
    let local = &governor.snapshot(wall())[0];
    assert_eq!(local.breaker.state, BreakerState::Closed);
}

#[tokio::test(start_paused = true)]
async fn backend_unavailable_trip_recovers_via_try_acquire() {
    recovers_through_a_try_acquire_probe(Outcome::BackendUnavailable, 1).await;
}

#[tokio::test(start_paused = true)]
async fn transient_failure_trip_recovers_via_try_acquire() {
    recovers_through_a_try_acquire_probe(Outcome::TransientFailure, 5).await;
}

#[tokio::test(start_paused = true)]
async fn a_failed_try_acquire_probe_reopens_with_a_doubled_cooldown() {
    let governor = split();
    trip(&governor, Outcome::BackendUnavailable, 1).await;
    tokio::time::advance(FIRST_OPEN).await;
    let prober = rewrite(&governor).unwrap();
    let probe = prober.try_begin_request().unwrap();
    assert!(probe.is_probe());
    probe.finish(Outcome::TransientFailure);
    drop(prober);
    let breaker = small(&governor).breaker;
    assert_eq!(breaker.state, BreakerState::Open);
    // 60 s doubled cooldown, jittered by 25 %.
    assert_eq!(breaker.retry_at, Some(wall() + TimeDelta::seconds(75)));
    assert!(rewrite(&governor).is_err());
}

#[tokio::test(start_paused = true)]
async fn a_probe_in_flight_refuses_other_try_acquire_callers() {
    let governor = split();
    trip(&governor, Outcome::BackendUnavailable, 1).await;
    tokio::time::advance(FIRST_OPEN).await;
    let prober = rewrite(&governor).unwrap();
    let _probe = prober.try_begin_request().unwrap();
    assert_eq!(
        rewrite(&governor).unwrap_err(),
        Refused::Unavailable { retry_at: None }
    );
}
