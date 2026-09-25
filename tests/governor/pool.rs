use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use kanade::infrastructure::llm::governor::{CallKind, Priority, Refused, Role, Ticket};
use tokio::time::Instant;

use crate::support::{LONG, governor, settle, snap, ticket};

type Log = Arc<Mutex<Vec<String>>>;

fn spawn_logged(
    governor: &Arc<kanade::infrastructure::llm::governor::Governor>,
    log: &Log,
    priority: Priority,
    who: &str,
) -> tokio::task::JoinHandle<()> {
    let governor = governor.clone();
    let log = log.clone();
    let who = who.to_string();
    tokio::spawn(async move {
        let permit = governor
            .acquire(Role::Chat, ticket(priority, &who), LONG)
            .await
            .expect("granted");
        log.lock().unwrap().push(who);
        drop(permit);
    })
}

#[tokio::test(start_paused = true)]
async fn priority_classes_are_served_highest_first() {
    let governor = governor(1, 600);
    let held = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "holder"), LONG)
        .await
        .unwrap();
    let log = Log::default();
    let order = [
        (Priority::FollowUp, "follow-up"),
        (Priority::Extraction, "extraction"),
        (Priority::ChatNew, "new chat"),
        (Priority::ChatRound, "chat round"),
        (Priority::Admin, "admin"),
    ];
    let tasks: Vec<_> = order
        .iter()
        .map(|(priority, who)| spawn_logged(&governor, &log, *priority, who))
        .collect();
    settle().await;
    assert_eq!(snap(&governor).queue.len(), 5);
    drop(held);
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(
        *log.lock().unwrap(),
        ["admin", "chat round", "new chat", "extraction", "follow-up"]
    );
}

#[tokio::test(start_paused = true)]
async fn waiters_in_one_class_are_fifo() {
    let governor = governor(1, 600);
    let held = governor
        .acquire(
            Role::Extraction,
            ticket(Priority::Extraction, "holder"),
            LONG,
        )
        .await
        .unwrap();
    let log = Log::default();
    let mut tasks = Vec::new();
    for who in ["first", "second", "third", "fourth"] {
        tasks.push(spawn_logged(&governor, &log, Priority::Extraction, who));
        settle().await;
    }
    drop(held);
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(*log.lock().unwrap(), ["first", "second", "third", "fourth"]);
}

#[tokio::test(start_paused = true)]
async fn permits_bound_concurrency() {
    let governor = governor(3, 600);
    let peak = Arc::new(Mutex::new((0u32, 0u32)));
    let tasks: Vec<_> = (0..8)
        .map(|i| {
            let governor = governor.clone();
            let peak = peak.clone();
            tokio::spawn(async move {
                let _permit = governor
                    .acquire(
                        Role::Chat,
                        ticket(Priority::ChatNew, &format!("m{i}")),
                        LONG,
                    )
                    .await
                    .unwrap();
                {
                    let mut p = peak.lock().unwrap();
                    p.0 += 1;
                    p.1 = p.1.max(p.0);
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                peak.lock().unwrap().0 -= 1;
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(peak.lock().unwrap().1, 3);
    assert_eq!(snap(&governor).permits.in_use, 0);
}

#[tokio::test(start_paused = true)]
async fn dropping_a_queued_waiter_frees_its_place() {
    let governor = governor(1, 600);
    let held = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "holder"), LONG)
        .await
        .unwrap();
    let log = Log::default();
    let cancelled = spawn_logged(&governor, &log, Priority::Admin, "cancelled");
    settle().await;
    let kept = spawn_logged(&governor, &log, Priority::FollowUp, "kept");
    settle().await;
    assert_eq!(snap(&governor).queue.len(), 2);
    cancelled.abort();
    settle().await;
    let queue = snap(&governor).queue;
    assert_eq!(queue.len(), 1);
    assert_eq!((queue[0].position, queue[0].who.as_str()), (1, "kept"));
    drop(held);
    kept.await.unwrap();
    assert_eq!(*log.lock().unwrap(), ["kept"]);
}

#[tokio::test(start_paused = true)]
async fn a_waiter_dropped_right_after_its_grant_gives_the_permit_back() {
    let governor = governor(1, 600);
    let held = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "holder"), LONG)
        .await
        .unwrap();
    let log = Log::default();
    let doomed = spawn_logged(&governor, &log, Priority::ChatNew, "doomed");
    settle().await;
    // The release grants the permit synchronously; the task never observes it.
    drop(held);
    assert_eq!(snap(&governor).permits.in_use, 1);
    doomed.abort();
    settle().await;
    assert_eq!(snap(&governor).permits.in_use, 0);
    assert!(log.lock().unwrap().is_empty());
    assert!(
        governor
            .try_acquire(Role::Chat, CallKind::Chat, "next")
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn queued_waits_time_out_and_leave_the_queue() {
    let governor = governor(1, 600);
    let _held = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "holder"), LONG)
        .await
        .unwrap();
    let start = Instant::now();
    let refused = governor
        .acquire(
            Role::Chat,
            ticket(Priority::ChatNew, "late"),
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
    assert_eq!(refused, Refused::Timeout);
    assert_eq!(start.elapsed(), Duration::from_secs(2));
    let snapshot = snap(&governor);
    assert!(snapshot.queue.is_empty());
    assert_eq!(snapshot.counters.shed_timeout, 1);
}

#[tokio::test(start_paused = true)]
async fn try_acquire_never_waits_or_jumps_the_queue() {
    let governor = governor(1, 600);
    let held = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
        .expect("free permit");
    assert_eq!(
        governor
            .try_acquire(Role::Rewrite, CallKind::PreScreen, "message 1")
            .unwrap_err(),
        Refused::Busy
    );
    assert_eq!(snap(&governor).counters.shed_busy, 1);
    assert_eq!(
        governor
            .acquire(
                Role::Rewrite,
                Ticket {
                    priority: Priority::FollowUp,
                    kind: CallKind::Rewrite,
                    who: "nudge".into(),
                },
                LONG,
            )
            .await
            .unwrap_err(),
        Refused::MustNotWait
    );
    drop(held);
    assert!(
        governor
            .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
            .is_ok()
    );
}

#[tokio::test(start_paused = true)]
async fn holders_are_tracked_and_released() {
    let governor = governor(2, 600);
    let a = governor
        .acquire(Role::Chat, ticket(Priority::ChatNew, "#hstar"), LONG)
        .await
        .unwrap();
    let _b = governor
        .try_acquire(Role::Rewrite, CallKind::Rewrite, "nudge")
        .unwrap();
    assert_eq!(a.alias(), crate::support::ALIAS);
    assert_eq!(a.group(), "local");
    let snapshot = snap(&governor);
    assert_eq!(snapshot.permits.in_use, 2);
    let holders: Vec<_> = snapshot
        .holders
        .iter()
        .map(|h| (h.kind, h.who.as_str()))
        .collect();
    assert_eq!(
        holders,
        [(CallKind::Chat, "#hstar"), (CallKind::Rewrite, "nudge")]
    );
    drop(a);
    assert_eq!(snap(&governor).permits.in_use, 1);
}
