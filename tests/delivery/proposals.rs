//! The tick expires proposals past their TTL exactly once and posts nothing
//! for them.

use chrono::TimeDelta;
use kanade::bot::delivery::{FixedClock, StoreRef};
use kanade::domain::drafts::{DraftEventKind, DraftStatus, ProposalSource};
use kanade::domain::history::{Actor, Origin};
use kanade::domain::ids::RandomIds;
use kanade::domain::proposals::{ChangeKind, ProposedChange};
use kanade::domain::schedule::{NewRun, RunSource, RunStatus};
use kanade::domain::scheduler::{ProposalRequest, SchedulerService, Supersede};

use crate::scenarios::{HOME, config, delivery, now, week, world};
use crate::support::{Store, on_both_stores};

async fn propose_at<S: Store>(
    store: &S,
    world: &crate::scenarios::World,
    at: chrono::DateTime<chrono::Utc>,
    run: &str,
) -> String {
    let mut service = SchedulerService::new(StoreRef(store), RandomIds, FixedClock(at));
    service
        .propose(
            ProposalRequest {
                change: ProposedChange {
                    run_id: Some(run.into()),
                    channel_id: Some(HOME.into()),
                    ..ProposedChange::new(ChangeKind::Cancel)
                },
                source: ProposalSource::Extraction,
                source_id: format!("log-{at}"),
                supersede: Supersede::Keep,
            },
            &config().policy,
            &world.roster,
        )
        .await
        .expect("propose")
        .proposal
        .id
}

async fn expire_proposals_once<S: Store>(store: &S) {
    let world = world();
    let created = now() - TimeDelta::hours(25);
    let mut service = SchedulerService::new(StoreRef(store), RandomIds, FixedClock(created));
    // Next boss week: no reminder falls due during the test.
    let run = service
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(HOME.into()),
            week_start: week() + TimeDelta::days(7),
            datetime: week() + TimeDelta::days(8),
            bosses: vec!["HFA".into()],
            participants: vec!["1001".into(), "1002".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .expect("run");
    let stale = propose_at(store, &world, created, &run).await;
    let fresh = propose_at(store, &world, now() - TimeDelta::hours(1), &run).await;

    let mut tick = delivery(store, &world, &world.fake);
    let first = tick.tick_at(now()).await.expect("tick");
    let calls = world.fake.calls().len();
    let second = tick
        .tick_at(now() + TimeDelta::minutes(1))
        .await
        .expect("tick");
    drop(tick);

    assert!(first.dispatch.sends.is_empty() && second.dispatch.sends.is_empty());
    assert_eq!(world.fake.calls().len(), calls, "the second tick posted");
    let loaded = store
        .load_draft(&stale)
        .await
        .expect("load")
        .expect("draft");
    assert_eq!(loaded.draft.status, DraftStatus::Expired);
    assert_eq!(loaded.draft.closed_by, Some(Actor::system("delivery")));
    let expired = store
        .draft_events(&stale)
        .await
        .expect("events")
        .iter()
        .filter(|event| event.kind == DraftEventKind::Expired)
        .count();
    assert_eq!(expired, 1, "expired exactly once");
    let fresh = store
        .load_draft(&fresh)
        .await
        .expect("load")
        .expect("draft");
    assert_eq!(fresh.draft.status, DraftStatus::Submitted);
    assert!(
        world.alerts.alerts().is_empty(),
        "{:?}",
        world.alerts.alerts()
    );
}

#[tokio::test]
async fn the_tick_expires_proposals_past_their_ttl_once_and_silently() {
    on_both_stores!(expire_proposals_once);
}
