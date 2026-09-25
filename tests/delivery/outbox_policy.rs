//! The drain's policies: undecodable rows never stop a tick, a backlog goes
//! stale instead of flooding the channels, and a source's notices keep their
//! order across a tick that leaves one pending.

use chrono::{DateTime, TimeDelta, Utc};
use kanade::bot::delivery::{AdminAlert, Delivery, SendOutcome};
use kanade::bot::transport::{Call, Op, RejectionKind, Step};
use kanade::domain::history::{Actor, ChangeMeta, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::notify::{DEFAULT_MAX_NOTICE_AGE, DrainReason, change_source};
use kanade::domain::schedule::{
    Change, ChangeSet, Notice, NoticeChange, RequestDecision, Run, RunSource, RunStatus,
};
use kanade::domain::scheduler::Scope;
use kanade::infrastructure::store::MemoryScheduleStore;

use crate::scenarios::{HOME, World, config, delivery, now, week, world};
use crate::support::{Store, TempDir, on_both_stores};

fn run(id: &str) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some(HOME.into()),
        week_start: week(),
        datetime: now() + TimeDelta::hours(3),
        bosses: vec!["Kalos".into()],
        participants: vec!["1001".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn merged(title: &str) -> Notice {
    Notice {
        change: NoticeChange::Merged {
            draft: "d-1".into(),
            version: 1,
            title: title.into(),
            run_ids: Vec::new(),
            fixed_ids: Vec::new(),
        },
        channel_id: Some(HOME.into()),
        listed: Vec::new(),
        via_portal: true,
    }
}

fn decided() -> Notice {
    Notice {
        change: NoticeChange::RequestDecided {
            request: "d-1".into(),
            decision: RequestDecision::Approved,
            reason: None,
        },
        channel_id: Some(HOME.into()),
        listed: vec!["1001".into()],
        via_portal: true,
    }
}

/// Commit a run row with `notices` at `at` (the outbox's `created_at`); its
/// source.
async fn enqueue<S: Store>(store: &S, id: &str, notices: Vec<Notice>, at: DateTime<Utc>) -> String {
    let revision = store.load(&Scope::All).await.expect("load").revision;
    let meta = ChangeMeta {
        origin: Origin::new(Actor::admin("test"), Surface::AdminPortal),
        at,
        notices: notices.iter().map(Notice::effect_kind).collect(),
        refs: Vec::new(),
        request_digest: None,
        expect: Default::default(),
        outbox: notices,
    };
    let committed = store
        .commit(
            revision,
            ChangeSet {
                changes: vec![Change::PutRun(run(id))],
            },
            meta,
        )
        .await
        .expect("commit")
        .expect("recorded");
    change_source(committed.seq)
}

fn posted(world: &World) -> Vec<String> {
    world
        .fake
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Create {
                message, outcome, ..
            } if outcome.is_delivered() => message.content,
            _ => None,
        })
        .collect()
}

async fn pending<S: Store>(store: &S) -> usize {
    store
        .pending_notices()
        .await
        .expect("pending")
        .notices
        .len()
}

// --- undecodable rows -------------------------------------------------------

async fn undecodable_is_skipped<S: Store>(store: &S, world: &World) {
    let mut delivery = delivery(store, world, &world.fake);
    for at in [now(), now() + TimeDelta::minutes(1)] {
        let report = delivery
            .tick_at(at)
            .await
            .expect("a bad row never aborts the tick");
        assert_eq!(report.notices.undecodable, 1);
    }
    let alerts: Vec<AdminAlert> = world
        .alerts
        .alerts()
        .into_iter()
        .filter(|alert| matches!(alert, AdminAlert::NoticeUndecodable { .. }))
        .collect();
    assert_eq!(alerts.len(), 1, "throttled: one alert per row per hour");
    assert_eq!(posted(world).len(), 1, "the good notice still posts");
    let left = store.pending_notices().await.expect("pending");
    assert_eq!(
        (left.notices.len(), left.undecodable.len()),
        (0, 1),
        "left pending"
    );
}

#[tokio::test]
async fn a_memory_undecodable_notice_is_skipped_and_alerted() {
    let store = MemoryScheduleStore::new();
    let bad = enqueue(&store, "r-1", vec![merged("bad")], now()).await;
    enqueue(&store, "r-2", vec![merged("good")], now()).await;
    store.corrupt_notice(&bad, 0, "payload damaged");
    undecodable_is_skipped(&store, &world()).await;
}

#[tokio::test]
async fn a_sqlite_undecodable_notice_is_skipped_and_alerted() {
    let dir = TempDir::new();
    let store = dir.open().await;
    enqueue(&store, "r-2", vec![merged("good")], now()).await;
    store.close().await.expect("close");
    // A row this build cannot read (a future payload version).
    let mut conn = sqlx::sqlite::SqliteConnectOptions::new().filename(&dir.config().db_path);
    conn = conn.foreign_keys(true);
    let mut conn = sqlx::ConnectOptions::connect(&conn).await.expect("raw");
    sqlx::query(
        "INSERT INTO notice_outbox (source, ordinal, effect_kind, payload, created_at)
         VALUES ('change:999', 0, 'notice.draft.merged', '{\"v\":9}', '2026-09-10T12:00:00+00:00')",
    )
    .execute(&mut conn)
    .await
    .expect("insert");
    sqlx::Connection::close(conn).await.expect("close raw");
    let store = dir.open().await;
    undecodable_is_skipped(&store, &world()).await;
    store.close().await.expect("close");
}

// --- age limit --------------------------------------------------------------

async fn a_backlog_goes_stale<S: Store>(store: &S) {
    let world = world();
    let old = now() - DEFAULT_MAX_NOTICE_AGE - TimeDelta::minutes(1);
    let source = enqueue(store, "r-1", vec![merged("old"), merged("older")], old).await;
    enqueue(
        store,
        "r-2",
        vec![merged("fresh")],
        now() - DEFAULT_MAX_NOTICE_AGE,
    )
    .await;
    let mut delivery = delivery(store, &world, &world.fake);
    let report = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(report.notices.stale, 2);
    assert_eq!(
        report.notices.sends.len(),
        1,
        "exactly the age limit still posts"
    );
    assert_eq!(posted(&world).len(), 1);
    let alerts: Vec<AdminAlert> = world
        .alerts
        .alerts()
        .into_iter()
        .filter(|alert| matches!(alert, AdminAlert::StaleNoticesRetired { .. }))
        .collect();
    assert_eq!(alerts, [AdminAlert::StaleNoticesRetired { count: 2 }]);
    let rows = store.outbox_notices().await.expect("outbox");
    let stale: Vec<_> = rows
        .iter()
        .filter(|row| row.source == source)
        .map(|row| row.drained_reason)
        .collect();
    assert_eq!(stale, [Some(DrainReason::Stale), Some(DrainReason::Stale)]);
    assert_eq!(pending(store).await, 0);
}

#[tokio::test]
async fn notices_older_than_the_age_limit_are_retired_unsent() {
    on_both_stores!(a_backlog_goes_stale);
}

async fn unroutable_goes_stale<S: Store>(store: &S) {
    let mut world = world();
    world.channels.clear();
    enqueue(store, "r-1", vec![merged("nowhere")], now()).await;
    let mut delivery = delivery(store, &world, &world.fake);
    let first = delivery.tick_at(now()).await.expect("tick");
    assert_eq!((first.notices.unroutable, first.notices.stale), (1, 0));
    let later = now() + DEFAULT_MAX_NOTICE_AGE + TimeDelta::minutes(1);
    let last = delivery.tick_at(later).await.expect("tick");
    assert_eq!((last.notices.unroutable, last.notices.stale), (0, 1));
    assert_eq!(pending(store).await, 0, "no longer pending forever");
    assert_eq!(world.fake.count(Op::Create), 0);
}

#[tokio::test]
async fn an_unroutable_notice_eventually_retires_as_stale() {
    on_both_stores!(unroutable_goes_stale);
}

// --- order within a source --------------------------------------------------

async fn released_summary_holds_the_requester_notice<S: Store>(store: &S) {
    let world = world();
    let source = enqueue(store, "r-1", vec![merged("request"), decided()], now()).await;
    world
        .fake
        .script(Op::Create, Step::Reject(RejectionKind::NotSent));
    let mut delivery = delivery(store, &world, &world.fake);
    let first = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(first.notices.sends.len(), 1);
    assert_eq!(
        first.notices.sends[0].send.outcome,
        SendOutcome::Released(RejectionKind::NotSent)
    );
    assert_eq!(first.notices.waiting, 1, "the requester notice waits");
    assert_eq!(world.fake.count(Op::Create), 1);
    let second = delivery
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick");
    let order: Vec<(String, i64)> = second
        .notices
        .sends
        .iter()
        .map(|send| (send.source.clone(), send.ordinal))
        .collect();
    assert_eq!(order, [(source.clone(), 0), (source, 1)]);
    let texts = posted(&world);
    assert_eq!(texts.len(), 2);
    assert!(
        texts[0].starts_with("📝 Schedule updated: request"),
        "{texts:?}"
    );
    assert!(texts[1].starts_with("📨"), "{texts:?}");
}

#[tokio::test]
async fn a_released_merge_summary_holds_back_its_requester_notice() {
    on_both_stores!(released_summary_holds_the_requester_notice);
}

async fn cap_holds_the_rest_of_a_source<S: Store>(store: &S) {
    let world = world();
    let source = enqueue(
        store,
        "r-1",
        vec![merged("one"), merged("two"), decided()],
        now(),
    )
    .await;
    let mut config = config();
    config.max_sends_per_tick = 1;
    let mut delivery = Delivery::new(
        store,
        RandomIds,
        &world.fake,
        &world.alerts,
        &world.roster,
        &world.channels,
        config,
    );
    let first = delivery.tick_at(now()).await.expect("tick");
    assert_eq!(
        (
            first.notices.sends.len(),
            first.notices.deferred,
            first.notices.waiting
        ),
        (1, 1, 1)
    );
    for minutes in [1, 2] {
        delivery
            .tick_at(now() + TimeDelta::minutes(minutes))
            .await
            .expect("tick");
    }
    let sent: Vec<i64> = store
        .outbox_notices()
        .await
        .expect("outbox")
        .into_iter()
        .filter(|row| row.source == source && row.drained_reason == Some(DrainReason::Journal))
        .map(|row| row.ordinal)
        .collect();
    assert_eq!(sent, [0, 1, 2]);
    assert!(posted(&world)[2].starts_with("📨"));
}

#[tokio::test]
async fn a_capped_notice_holds_back_the_rest_of_its_source() {
    on_both_stores!(cap_holds_the_rest_of_a_source);
}
