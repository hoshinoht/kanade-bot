//! Simulated backend outage with 50 queued calls: nothing is sent while open,
//! one probe goes first, and the drain respects both the permit ramp and the
//! rate ceiling.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use kanade::infrastructure::llm::governor::{BreakerState, CallKind, Outcome, Priority, Role};
use tokio::time::Instant;

use crate::support::{LONG, governor, settle, snap, ticket};

#[derive(Default)]
struct Trace {
    in_flight: u32,
    /// (sent at, in flight including this one, was probe)
    sends: Vec<(Instant, u32, bool)>,
    closed_at: Option<Instant>,
}

#[tokio::test(start_paused = true)]
async fn fifty_queued_calls_drain_without_a_burst() {
    const PER_MIN: u32 = 60;
    const BURST: usize = 4;
    let governor = governor(4, PER_MIN);
    let holders: Vec<_> = (0..4)
        .map(|_| {
            governor
                .try_acquire(Role::Chat, CallKind::Chat, "holder")
                .unwrap()
        })
        .collect();
    let trace = Arc::new(Mutex::new(Trace::default()));
    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let governor = governor.clone();
            let trace = trace.clone();
            tokio::spawn(async move {
                let permit = governor
                    .acquire(
                        Role::Extraction,
                        ticket(Priority::Extraction, &format!("run {i}")),
                        LONG,
                    )
                    .await
                    .expect("queued call survives the outage");
                let attempt = permit.begin_request(LONG).await.expect("admitted");
                {
                    let mut t = trace.lock().unwrap();
                    t.in_flight += 1;
                    let entry = (Instant::now(), t.in_flight, attempt.is_probe());
                    t.sends.push(entry);
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
                let probe = attempt.is_probe();
                attempt.finish(Outcome::Success);
                let mut t = trace.lock().unwrap();
                t.in_flight -= 1;
                if probe {
                    t.closed_at = Some(Instant::now());
                }
            })
        })
        .collect();
    settle().await;
    assert_eq!(snap(&governor).queue.len(), 50);

    let opened = Instant::now();
    holders[0]
        .begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::BackendUnavailable);
    drop(holders);
    settle().await;
    assert_eq!(snap(&governor).breaker.state, BreakerState::Open);
    assert_eq!(snap(&governor).permits.in_use, 0);

    for task in tasks {
        task.await.unwrap();
    }
    let trace = trace.lock().unwrap();
    assert_eq!(trace.sends.len(), 50);
    let (first, _, probe) = trace.sends[0];
    assert!(probe, "the probe goes first");
    assert!(first - opened >= Duration::from_secs(30));
    assert_eq!(trace.sends.iter().filter(|s| s.2).count(), 1);
    let closed = trace.closed_at.expect("probe closed the breaker");

    for (sent, in_flight, probe) in &trace.sends[1..] {
        assert!(!probe);
        assert!(*sent >= closed);
        let ramp = 1 + (*sent - closed).as_secs() / 5;
        assert!(
            u64::from(*in_flight) <= ramp.min(4),
            "{in_flight} in flight at ramp {ramp}"
        );
    }
    for i in 0..trace.sends.len() {
        for j in i..trace.sends.len() {
            let span = trace.sends[j].0 - trace.sends[i].0;
            let allowed = BURST as u64 + span.as_secs() * u64::from(PER_MIN) / 60;
            assert!(
                ((j - i) as u64) < allowed,
                "burst between sends {i} and {j}"
            );
        }
    }
    let snapshot = snap(&governor);
    assert_eq!(snapshot.breaker.state, BreakerState::Closed);
    assert_eq!(snapshot.counters.requests, 51);
    assert_eq!(snapshot.permits.in_use, 0);
}
