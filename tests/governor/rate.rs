use std::time::Duration;

use kanade::infrastructure::llm::governor::{CallKind, Outcome, Priority, Refused, Role};
use tokio::time::Instant;

use crate::support::{LONG, build, governor, single, snap, ticket};

#[tokio::test(start_paused = true)]
async fn bursts_are_smoothed_even_with_free_permits() {
    // 4 permits, 60/min, burst defaults to the permit count.
    let governor = governor(4, 60);
    let permit = governor
        .acquire(
            Role::Extraction,
            ticket(Priority::Extraction, "burst"),
            LONG,
        )
        .await
        .unwrap();
    let start = Instant::now();
    let mut sent = Vec::new();
    for _ in 0..10 {
        let attempt = permit.begin_request(LONG).await.unwrap();
        sent.push(start.elapsed());
        attempt.finish(Outcome::Success);
    }
    let expected: Vec<_> = [0, 0, 0, 0, 1, 2, 3, 4, 5, 6]
        .into_iter()
        .map(Duration::from_secs)
        .collect();
    assert_eq!(sent, expected);
}

#[tokio::test(start_paused = true)]
async fn concurrent_holders_share_the_ceiling_in_reservation_order() {
    let governor = governor(4, 60);
    let start = Instant::now();
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let governor = governor.clone();
            tokio::spawn(async move {
                let permit = governor
                    .acquire(
                        Role::Chat,
                        ticket(Priority::ChatNew, &format!("m{i}")),
                        LONG,
                    )
                    .await
                    .unwrap();
                let attempt = permit.begin_request(LONG).await.unwrap();
                let at = start.elapsed();
                attempt.finish(Outcome::Success);
                at
            })
        })
        .collect();
    let mut sent = Vec::new();
    for task in tasks {
        sent.push(task.await.unwrap());
    }
    sent.sort();
    for (index, at) in sent.iter().enumerate() {
        // Never more than burst + elapsed × rate requests by any instant.
        assert!(
            (index as u64) < 4 + at.as_secs(),
            "request {index} at {at:?}"
        );
    }
    assert_eq!(sent.last().copied(), Some(Duration::from_secs(4)));
}

#[tokio::test(start_paused = true)]
async fn a_wait_beyond_the_callers_bound_is_refused_without_spending_a_token() {
    let mut config = single(2, 6);
    config.groups[0].burst = Some(1);
    let governor = build(&config);
    let permit = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
        .unwrap();
    permit.try_begin_request().unwrap().finish(Outcome::Success);
    assert_eq!(
        permit.try_begin_request().unwrap_err(),
        Refused::RateLimited {
            wait: Duration::from_secs(10)
        }
    );
    assert_eq!(
        permit
            .begin_request(Duration::from_secs(9))
            .await
            .unwrap_err(),
        Refused::RateLimited {
            wait: Duration::from_secs(10)
        }
    );
    let start = Instant::now();
    permit
        .begin_request(Duration::from_secs(10))
        .await
        .unwrap()
        .finish(Outcome::Success);
    assert_eq!(start.elapsed(), Duration::from_secs(10));
    let snapshot = snap(&governor);
    assert_eq!(snapshot.counters.shed_rate, 2);
    assert_eq!(snapshot.counters.requests, 2);
}

#[tokio::test(start_paused = true)]
async fn the_bucket_refills_to_its_capacity_only() {
    let mut config = single(1, 2);
    config.groups[0].burst = Some(12);
    let governor = build(&config);
    let permit = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
        .unwrap();
    for _ in 0..3 {
        permit.try_begin_request().unwrap().finish(Outcome::Success);
    }
    assert_eq!(snap(&governor).rate.available, 9);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(snap(&governor).rate.available, 11);
    tokio::time::advance(Duration::from_secs(3_600)).await;
    let rate = snap(&governor).rate;
    assert_eq!(
        (rate.available, rate.capacity, rate.refill_per_min),
        (12, 12, 2)
    );
}
