use std::time::Duration;

use chrono::TimeDelta;
use kanade::infrastructure::llm::governor::{
    BreakerState, CallKind, GovernorConfig, GovernorPolicy, Outcome, PermitUsage, Priority,
    QueuedCall, RateLevel, RetryLevel, Role,
};

use crate::support::{LONG, build, group, roles, settle, ticket, wall};

#[tokio::test(start_paused = true)]
async fn snapshot_matches_the_backend_group_dto() {
    let governor = build(&GovernorConfig {
        groups: vec![
            group("gpu", 1, 30, &["local-a", "local-b"]),
            group("cloud", 8, 120, &["cloud-a"]),
        ],
        roles: roles("local-a", false),
        policy: GovernorPolicy::default(),
    });
    let held = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "#hstar"), LONG)
        .await
        .unwrap();
    held.begin_request(LONG)
        .await
        .unwrap()
        .finish(Outcome::TransientFailure);
    let queued = [
        (Priority::Extraction, CallKind::Rescan, "admin token"),
        (
            Priority::Extraction,
            CallKind::Extraction,
            "#hstar-party burst",
        ),
        (Priority::ChatNew, CallKind::Chat, "Kanade fan"),
    ];
    let mut tasks = Vec::new();
    for (priority, kind, who) in queued {
        let governor = governor.clone();
        tasks.push(tokio::spawn(async move {
            let ticket = kanade::infrastructure::llm::governor::Ticket {
                priority,
                kind,
                who: who.into(),
            };
            governor
                .acquire(Role::Extraction, ticket, LONG)
                .await
                .map(drop)
        }));
        settle().await;
        tokio::time::advance(Duration::from_secs(4)).await;
    }

    let groups = governor.snapshot(wall());
    assert_eq!(
        groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(),
        ["gpu", "cloud"]
    );
    let gpu = &groups[0];
    assert_eq!(gpu.backend, "gpu backend");
    assert_eq!(gpu.models, ["local-a", "local-b"]);
    assert_eq!(
        gpu.permits,
        PermitUsage {
            in_use: 1,
            total: 1
        }
    );
    assert_eq!(
        gpu.queue,
        [
            QueuedCall {
                position: 1,
                kind: CallKind::Chat,
                who: "Kanade fan".into(),
                waiting_s: 4,
            },
            QueuedCall {
                position: 2,
                kind: CallKind::Rescan,
                who: "admin token".into(),
                waiting_s: 12,
            },
            QueuedCall {
                position: 3,
                kind: CallKind::Extraction,
                who: "#hstar-party burst".into(),
                waiting_s: 8,
            },
        ]
    );
    assert_eq!(gpu.queue[1].kind.as_str(), "rescan");
    assert_eq!(gpu.holders[0].held_s, 12);
    // 1 token spent, 12 s × 30/min refilled back to the 1-token burst.
    assert_eq!(
        gpu.rate,
        RateLevel {
            available: 1,
            capacity: 1,
            refill_per_min: 30
        }
    );
    assert_eq!(
        gpu.retry,
        RetryLevel {
            remaining: 1,
            capacity: 1
        }
    );
    assert_eq!(gpu.breaker.state, BreakerState::Closed);
    assert_eq!(gpu.breaker.state.as_str(), "closed");
    assert_eq!(gpu.breaker.failures, 1);
    assert_eq!(gpu.breaker.since, wall() - TimeDelta::seconds(12));
    assert_eq!(gpu.breaker.retry_at, None);
    assert_eq!(gpu.counters.transient_failures, 1);

    let cloud = &groups[1];
    assert_eq!(
        cloud.permits,
        PermitUsage {
            in_use: 0,
            total: 8
        }
    );
    assert!(cloud.queue.is_empty() && cloud.holders.is_empty());
    assert_eq!(cloud.rate.available, 8);

    drop(held);
    for task in tasks {
        task.await.unwrap().unwrap();
    }
}
