//! Notices follow the change's surface (parent decision 2026-09-25, v4
//! parity): a move made in Discord has no "(via portal)" mark and a Discord
//! `/fixed edit` announces nothing; the same changes from the portal keep
//! the mark and the announcement.

use chrono::{NaiveTime, Weekday};
use serde_json::json;

use kanade::bot::delivery::FixedClock;
use kanade::domain::history::{Actor, ChangeHistory, Origin, Surface};
use kanade::domain::ids::RandomIds;
use kanade::domain::members::Roster;
use kanade::domain::notify::{NoticeOutbox, OutboxNotice};
use kanade::domain::schedule::{
    FixedEdit, FixedEditChoices, FixedEditRequest, NoticeChange, ReminderPolicy, SchedulePolicy,
};
use kanade::domain::scheduler::SchedulerService;

use super::{ALICE, BOB, F_KALOS, R_KALOS, Slash, now, opt, sub, utc};

fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: chrono_tz::Asia::Kuala_Lumpur,
            ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

fn portal() -> Origin {
    Origin::new(Actor::admin("9001"), Surface::AdminPortal)
}

async fn written(slash: &Slash) -> Vec<OutboxNotice> {
    slash.store.outbox_notices().await.unwrap()
}

#[tokio::test]
async fn a_discord_amend_is_unmarked_and_a_portal_amend_is_marked() {
    let slash = Slash::new().await;
    slash
        .run(
            ALICE,
            "amend",
            json!([opt("run_id", R_KALOS), opt("to", "wed 21:30")]),
        )
        .await;
    let discord = written(&slash).await;
    let [moved] = discord.as_slice() else {
        panic!("one notice: {discord:?}");
    };
    assert!(matches!(moved.notice.change, NoticeChange::RunMoved { .. }));
    assert!(!moved.notice.via_portal, "v4's /amend had no portal mark");

    SchedulerService::new(&*slash.store, RandomIds, FixedClock(now()))
        .as_origin(portal())
        .amend_run(R_KALOS, utc(9, 30, 14, 0), &policy())
        .await
        .unwrap();
    let all = written(&slash).await;
    let portal_notice = all.last().unwrap();
    assert!(matches!(
        portal_notice.notice.change,
        NoticeChange::RunMoved { .. }
    ));
    assert!(portal_notice.notice.via_portal);
}

#[tokio::test]
async fn a_discord_fixed_edit_announces_nothing_and_a_portal_edit_does() {
    let slash = Slash::new().await;
    let head = slash.store.history_head().await.unwrap().seq;
    let reply = slash
        .run(
            BOB,
            "fixed",
            sub("edit", json!([opt("id", "ffff"), opt("time", "23:00")])),
        )
        .await;
    assert!(reply.starts_with("✅ Updated."), "{reply}");
    let record = slash.store.load_change(head + 1).await.unwrap().unwrap();
    assert_eq!(record.origin.surface, Surface::Discord);
    assert!(record.notices.is_empty(), "{:?}", record.notices);
    assert!(
        written(&slash).await.is_empty(),
        "v4's slash edit posted nothing"
    );

    SchedulerService::new(&*slash.store, RandomIds, FixedClock(now()))
        .as_origin(portal())
        .apply_fixed_edit(
            &FixedEditRequest {
                fixed_id: F_KALOS.into(),
                edit: FixedEdit {
                    time: NaiveTime::from_hms_opt(22, 30, 0),
                    ..FixedEdit::default()
                },
                choices: FixedEditChoices::UpdateAll,
            },
            &Roster::new(),
            &policy(),
        )
        .await
        .unwrap();
    let portal = written(&slash).await;
    let [changed] = portal.as_slice() else {
        panic!("one notice: {portal:?}");
    };
    assert!(matches!(
        changed.notice.change,
        NoticeChange::FixedChanged { .. }
    ));
    assert!(changed.notice.via_portal);
}
