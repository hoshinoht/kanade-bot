//! The tick expires past-week drafts and never fails when expiry does.

use chrono::TimeDelta;
use kanade::bot::delivery::AdminAlert;
use kanade_domain::drafts::{
    DraftChange, DraftCreated, DraftEventKind, DraftKind, DraftStatus, DraftUpdate, DraftWrite,
    NewDraft, StoredDraft,
};
use kanade_domain::history::Actor;
use kanade_store::SqliteStore;

use crate::scenarios::{delivery, now, previous_week, week, world};
use crate::support::Store;

/// An open administrator draft, as older stores hold them.
async fn open_draft<S: Store>(store: &S, id: &str, title: &str) -> StoredDraft {
    let (snapshot, base) = store.snapshot_with_head().await.expect("head");
    let created = store
        .create_draft(NewDraft {
            id: id.into(),
            kind: DraftKind::Admin,
            title: title.into(),
            author: Actor::admin("root"),
            base,
            base_revision: snapshot.revision,
            request_type: None,
            subject: None,
            at: now(),
            request: None,
            submit: None,
        })
        .await
        .expect("draft");
    let DraftCreated::Created(draft) = created else {
        panic!("draft not created: {created:?}");
    };
    draft
}

async fn expire_past_week_drafts<S: Store>(store: &S) {
    let world = world();
    let past = open_draft(store, "draft-1", "past").await;
    let weekly = open_draft(store, "draft-2", "weekly").await;
    // The store is handed a derived week directly (the tick only reads it).
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
    expire_past_week_drafts(&SqliteStore::open_temp().await).await;
}

#[tokio::test]
async fn a_failed_expiry_never_aborts_the_tick() {
    let store = SqliteStore::open_temp().await;
    let world = world();
    let dir = store.temp_dir();
    dir.raw("DROP TABLE drafts;").await.expect("drop");
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

#[tokio::test]
async fn a_merged_notice_renders_its_title_and_runs() {
    use kanade::bot::delivery::render_notice;
    use kanade_domain::notify::{DeliverySettings, plan_notice};
    use kanade_domain::schedule::{Notice, NoticeChange};

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
        },
    )
    .expect("plannable");
    assert_eq!(intent.channel_id, "222");
    let message = render_notice(
        &notice,
        &intent,
        &kanade_domain::schedule::ScheduleSnapshot::default(),
        &world.roster,
        chrono_tz::Asia::Kuala_Lumpur,
        false,
        &kanade::cards::CardKit::default(),
    )
    .expect("renders");
    let text = message.content.expect("text");
    assert_eq!(text, "📝 Schedule updated: retime the raid\n-# via portal");
}
