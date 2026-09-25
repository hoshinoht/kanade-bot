//! The tick expires past-week drafts and never fails when expiry does.

use chrono::TimeDelta;
use kanade::bot::delivery::{AdminAlert, FixedClock, StoreRef};
use kanade::domain::drafts::{DraftChange, DraftEventKind, DraftStatus, DraftUpdate, DraftWrite};
use kanade::domain::history::Actor;
use kanade::domain::ids::IdGenerator;
use kanade::domain::scheduler::SchedulerService;

use crate::scenarios::{delivery, now, previous_week, week, world};
use crate::support::{Store, TempDir, on_both_stores};

#[derive(Default)]
struct Counter(u64);

impl IdGenerator for Counter {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("draft-{n}", n = self.0)
    }
}

async fn expire_past_week_drafts<S: Store>(store: &S) {
    let world = world();
    let mut drafts = SchedulerService::new(StoreRef(store), Counter::default(), FixedClock(now()));
    let past = drafts
        .create_draft("root", "past", None)
        .await
        .expect("draft");
    let weekly = drafts
        .create_draft("root", "weekly", None)
        .await
        .expect("draft");
    // The service derives the week from staged run operations; here the
    // store is handed that derived week directly (the tick only reads it).
    let scoped = store
        .update_draft(DraftUpdate {
            draft_id: past.id.clone(),
            expected_version: past.version,
            actor: Actor::admin("root"),
            at: now(),
            change: DraftChange::ReplaceOps {
                ops: Vec::new(),
                event: DraftEventKind::OpAdded,
                ord: 0,
                expires_week: Some(previous_week()),
            },
        })
        .await
        .expect("scope");
    assert!(matches!(scoped, DraftWrite::Written(_)));

    let mut tick = delivery(store, &world, &world.fake);
    tick.tick_at(now()).await.expect("tick");
    tick.tick_at(now() + TimeDelta::hours(1))
        .await
        .expect("tick");
    drop(tick);

    let past = store
        .load_draft(&past.id)
        .await
        .expect("load")
        .expect("draft");
    assert_eq!(past.draft.status, DraftStatus::Expired);
    let weekly = store
        .load_draft(&weekly.id)
        .await
        .expect("load")
        .expect("draft");
    assert_eq!(weekly.draft.status, DraftStatus::Open);
    assert!(
        world
            .alerts
            .alerts()
            .iter()
            .all(|alert| !matches!(alert, AdminAlert::DraftExpiryFailed { .. })),
        "expiry raised no alert: {:?}",
        world.alerts.alerts()
    );
    assert!(previous_week() < week(), "the fixture week is past");
}

#[tokio::test]
async fn the_tick_expires_past_week_drafts() {
    on_both_stores!(expire_past_week_drafts);
}

#[tokio::test]
async fn a_failed_expiry_never_aborts_the_tick() {
    let dir = TempDir::new();
    let store = dir.open().await;
    let world = world();
    raw(&dir, "DROP TABLE drafts;").await;
    let mut tick = delivery(&store, &world, &world.fake);
    tick.tick_at(now()).await.expect("the tick still runs");
    assert!(
        world
            .alerts
            .alerts()
            .iter()
            .any(|alert| matches!(alert, AdminAlert::DraftExpiryFailed { .. })),
        "{:?}",
        world.alerts.alerts()
    );
    drop(tick);
    store.close().await.expect("close");
}

async fn raw(dir: &TempDir, sql: &str) {
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Connection};

    let mut conn = SqliteConnectOptions::new()
        .filename(dir.config().db_path)
        .connect()
        .await
        .expect("raw connection");
    sqlx::raw_sql(sql)
        .execute(&mut conn)
        .await
        .expect("raw sql");
    conn.close().await.expect("close");
}

#[tokio::test]
async fn a_merged_notice_renders_its_title_and_runs() {
    use kanade::bot::delivery::render_notice;
    use kanade::domain::notify::{DeliverySettings, plan_notice};
    use kanade::domain::schedule::{Notice, NoticeChange};

    let world = world();
    let notice = Notice {
        change: NoticeChange::Merged {
            draft: "draft-1".into(),
            version: 2,
            title: "retime the raid".into(),
            run_ids: vec!["run-1".into()],
            fixed_ids: Vec::new(),
        },
        channel_id: Some("222".into()),
        listed: Vec::new(),
        via_portal: true,
    };
    let intent = plan_notice(
        &notice,
        &world.roster,
        &world.channels,
        DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
            attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
        },
    )
    .expect("plannable");
    assert_eq!(intent.channel_id, "222");
    let message = render_notice(
        &notice,
        &intent,
        &kanade::domain::schedule::ScheduleSnapshot::default(),
        &world.roster,
        chrono_tz::Asia::Kuala_Lumpur,
        false,
    )
    .expect("renders");
    let text = message.content.expect("text");
    assert_eq!(text, "📝 Schedule updated: retime the raid\n_(via portal)_");
}
