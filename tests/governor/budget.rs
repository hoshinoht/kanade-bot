use std::time::Duration;

use kanade::infrastructure::llm::governor::{CallKind, Outcome, Refused, Role};

use crate::support::{LONG, build, governor, single, snap};

#[tokio::test(start_paused = true)]
async fn a_quiet_group_may_still_retry_once() {
    let governor = governor(1, 600);
    let permit = governor
        .try_acquire(Role::Chat, CallKind::Chat, "m")
        .unwrap();
    permit
        .begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::TransientFailure);
    permit
        .begin_retry(LONG)
        .await
        .unwrap()
        .finish(Outcome::TransientFailure);
    assert_eq!(
        permit.begin_retry(LONG).await.unwrap_err(),
        Refused::RetryBudgetExhausted
    );
    let snapshot = snap(&governor);
    assert_eq!((snapshot.retry.remaining, snapshot.retry.capacity), (0, 1));
    assert_eq!(snapshot.counters.retries_denied, 1);
}

#[tokio::test(start_paused = true)]
async fn retries_are_capped_at_fifteen_percent_of_recent_requests() {
    let mut config = single(1, 6_000);
    config.groups[0].burst = Some(1_000);
    let governor = build(&config);
    let permit = governor
        .try_acquire(Role::Chat, CallKind::Chat, "m")
        .unwrap();
    for _ in 0..40 {
        permit
            .begin_request(LONG)
            .await
            .unwrap()
            .finish(Outcome::Success);
    }
    let retry = snap(&governor).retry;
    assert_eq!((retry.remaining, retry.capacity), (6, 6));
    for _ in 0..6 {
        permit
            .begin_retry(LONG)
            .await
            .unwrap()
            .finish(Outcome::Success);
    }
    assert_eq!(
        permit.begin_retry(LONG).await.unwrap_err(),
        Refused::RetryBudgetExhausted
    );
    // Retries are not requests: they never enlarge their own budget.
    assert_eq!(snap(&governor).retry.capacity, 6);
    assert_eq!(snap(&governor).counters.retries, 6);
}

#[tokio::test(start_paused = true)]
async fn the_window_slides() {
    let mut config = single(1, 6_000);
    config.groups[0].burst = Some(1_000);
    let governor = build(&config);
    let permit = governor
        .try_acquire(Role::Chat, CallKind::Chat, "m")
        .unwrap();
    for _ in 0..20 {
        permit
            .begin_request(LONG)
            .await
            .unwrap()
            .finish(Outcome::Success);
    }
    for _ in 0..3 {
        permit
            .begin_retry(LONG)
            .await
            .unwrap()
            .finish(Outcome::Success);
    }
    assert_eq!(snap(&governor).retry.remaining, 0);
    tokio::time::advance(Duration::from_secs(59)).await;
    assert_eq!(snap(&governor).retry.remaining, 0);
    tokio::time::advance(Duration::from_secs(1)).await;
    let retry = snap(&governor).retry;
    assert_eq!((retry.remaining, retry.capacity), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn a_denied_retry_spends_no_rate_token() {
    let governor = governor(4, 60);
    let permit = governor
        .try_acquire(Role::Chat, CallKind::Chat, "m")
        .unwrap();
    permit
        .begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::Success);
    permit
        .begin_retry(LONG)
        .await
        .unwrap()
        .finish(Outcome::Success);
    assert_eq!(snap(&governor).rate.available, 2);
    assert!(permit.begin_retry(LONG).await.is_err());
    assert_eq!(snap(&governor).rate.available, 2);
}
