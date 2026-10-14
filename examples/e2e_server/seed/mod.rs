//! What the template store holds. So far only what `e2e/harness/member.spec`
//! reads: the portal open, the fake Discord users as guild members, one run
//! next boss week that Mikan is on, and one weekly timing she owns with Yuzu.
//! Written over the composed state, so every later generation (composed
//! afresh from a copy) reads them as a restarted server would.

use std::sync::Arc;

use chrono::{Duration, NaiveTime, Weekday};
use kanade::{
    api::{state::ApiState, write::ApiClock},
    domain::{
        history::{Actor, Origin, Surface},
        ids::RandomIds,
        members::{GatewayMember, MemberStore},
        schedule::{NewFixedRun, NewRun, RunSource, RunStatus, utc_instant},
        scheduler::SchedulerService,
        settings::{Section, SelfService, save_section},
    },
    infrastructure::store::SqliteStore,
};

use crate::{Fail, fakes::discord};

pub async fn template(store: Arc<SqliteStore>, state: &ApiState) -> Result<(), Fail> {
    let portal = SelfService {
        public_portal: true,
        ..SelfService::default()
    };
    save_section(&*store, &Section::SelfService(portal)).await?;
    for (choice, has_role) in discord::MEMBERS {
        let user = discord::user(choice);
        store
            .apply_gateway(GatewayMember {
                user_id: user.id.clone(),
                display_name: Some(user.display()),
                has_role,
                ..GatewayMember::default()
            })
            .await?;
    }
    schedule(store, state).await
}

/// Through the composed policy and clock.
async fn schedule(store: Arc<SqliteStore>, state: &ApiState) -> Result<(), Fail> {
    let policy = &state.policy;
    let mikan = discord::user("eligible").id;
    let yuzu = discord::user("partner").id;
    let week_start = utc_instant(&policy.week_of(&(state.clock)())?)? + Duration::days(7);
    let mut scheduler = SchedulerService::new(store, RandomIds, ApiClock(state.clock.clone()))
        .with_attendance(policy.attendance);
    let origin = Origin::new(Actor::system("e2e_server"), Surface::Cli);
    scheduler
        .as_origin(origin.clone())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: None,
            week_start,
            // Day one at noon.
            datetime: week_start + Duration::days(1) + Duration::hours(12),
            bosses: vec!["HSeren".into()],
            participants: vec![mikan.clone()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await?;
    scheduler
        .as_origin(origin)
        .add_fixed_run(NewFixedRun {
            owner_id: mikan.clone(),
            channel_id: None,
            bosses: vec!["HSeren".into()],
            weekday: Weekday::Wed,
            time: NaiveTime::from_hms_opt(21, 0, 0).ok_or("21:00")?,
            participants: vec![mikan, yuzu],
            note: None,
            owner_pinned: false,
        })
        .await?;
    Ok(())
}
