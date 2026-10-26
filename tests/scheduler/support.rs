//! Fixtures shared by the hand-written scheduler tests: a Kuala
//! Lumpur guild (members 1001–1004 watching channels 222 and 333) with its
//! policy, readable ids, draft-operation builders and the weekly-timing
//! schedules most tests start from.

use chrono::{DateTime, FixedOffset, NaiveTime, TimeDelta, TimeZone, Utc, Weekday};
use chrono_tz::Asia::Kuala_Lumpur;
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::drafts::{DraftOp, Target};
use kanade_domain::history::Origin;
use kanade_domain::ids::{IdGenerator, RandomIds};
use kanade_domain::members::{Directory, Member};
use kanade_domain::notify::ChannelDirectory;
use kanade_domain::schedule::{
    NewFixedRun, ReminderPolicy, RsvpSource, RsvpState, RunSource, RunStatus, SchedulePolicy,
    ScheduleSnapshot,
};
use kanade_domain::scheduler::{Attributed, Clock, ScheduleStore, SchedulerService, Scope};

use crate::common::TestClock;

/// `id-0001`, `id-0002`, …
#[derive(Default)]
pub struct CountingIds(u32);

impl IdGenerator for CountingIds {
    fn new_id(&mut self) -> String {
        self.0 += 1;
        format!("id-{:04}", self.0)
    }
}

/// Members 1001–1004 (with the bossing role and a display name); channels
/// 222 and 333 are watched.
pub struct Guild;

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        ["1001", "1002", "1003", "1004"]
            .contains(&user_id)
            .then(|| Member {
                user_id: user_id.to_owned(),
                display_name: Some(format!("member {user_id}")),
                has_role: true,
                ..Member::default()
            })
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        ["222", "333"].contains(&channel_id)
    }
}

pub struct AnyChannel;

impl ChannelDirectory for AnyChannel {
    fn is_reachable(&self, _channel_id: &str) -> bool {
        true
    }
}

/// Kuala Lumpur wall clock in 2026: days from the 27th are August, earlier
/// days September.
pub fn kl(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
    let month = if d >= 27 { 8 } else { 9 };
    FixedOffset::east_opt(8 * 3600)
        .unwrap()
        .with_ymd_and_hms(2026, month, d, h, mi, 0)
        .unwrap()
}

pub fn utc(at: DateTime<FixedOffset>) -> DateTime<Utc> {
    at.with_timezone(&Utc)
}

/// Thu 27 Aug 01:00, just after the reset.
pub fn now() -> DateTime<Utc> {
    utc(kl(27, 1, 0))
}

pub fn hm(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
}

/// Pings at 09:00 plus 60 and 15 minutes before; the boss week resets
/// Thursday 00:00.
pub fn policy() -> SchedulePolicy {
    SchedulePolicy::new(
        ReminderPolicy {
            zone: Kuala_Lumpur,
            ping_time: hm(9, 0),
            countdowns: vec![60, 15],
        },
        Weekday::Thu,
        NaiveTime::MIN,
    )
}

pub fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

/// An HFA weekly timing owned by 1001.
pub fn weekly(channel: &str, weekday: Weekday, time: NaiveTime, party: &[&str]) -> NewFixedRun {
    NewFixedRun {
        owner_pinned: false,
        owner_id: "1001".into(),
        channel_id: Some(channel.into()),
        bosses: vec!["HFA".into()],
        weekday,
        time,
        participants: owned(party),
        note: None,
    }
}

pub async fn snapshot<S: ScheduleStore, I: IdGenerator, C: Clock>(
    service: &SchedulerService<S, I, C>,
) -> ScheduleSnapshot {
    service.store().load(&Scope::All).await.unwrap()
}

/// `service` attributing its next mutation to the tests.
pub fn by_tests<S: ScheduleStore, I: IdGenerator, C: Clock>(
    service: &mut SchedulerService<S, I, C>,
) -> Attributed<'_, S, I, C> {
    service.as_origin(Origin::for_tests())
}

pub fn existing(id: &str) -> Target {
    Target::Existing(id.to_owned())
}

pub fn amend(run: &str, to: DateTime<FixedOffset>) -> DraftOp {
    DraftOp::AmendRun {
        run: existing(run),
        to: utc(to),
    }
}

pub fn swap(run: &str, remove: &[&str], add: &[&str]) -> DraftOp {
    DraftOp::SwapParticipants {
        run: existing(run),
        remove: owned(remove),
        add: owned(add),
        via_portal: true,
    }
}

pub fn rsvp(run: &str, user: &str, state: RsvpState) -> DraftOp {
    DraftOp::SetRsvp {
        run: existing(run),
        user_id: user.to_owned(),
        state,
        source: RsvpSource::Chat,
    }
}

/// A planned Kalos run for 1001 in 222 on Sat 29 Aug 20:00.
pub fn kalos_run(fixed: Option<Target>, week_start: DateTime<Utc>) -> DraftOp {
    DraftOp::CreateRun {
        fixed,
        channel_id: Some("222".into()),
        week_start,
        datetime: utc(kl(29, 20, 0)),
        bosses: vec!["Kalos".into()],
        participants: vec!["1001".into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
    }
}

/// Weekly Mon 21:30 in 222 (party 1001, 1002) with runs for 31 Aug, 7 Sep
/// and 14 Sep; the 7 Sep run is amended to Wed 9 Sep 21:00.
pub struct OneTiming {
    pub service: SchedulerService<SqliteStore, CountingIds, TestClock>,
    pub clock: TestClock,
    pub fixed: String,
    pub runs: [String; 3],
}

pub async fn one_timing() -> OneTiming {
    let clock = TestClock::new(kl(27, 1, 0));
    let mut service = SchedulerService::new(
        SqliteStore::open_temp().await,
        CountingIds::default(),
        clock.clone(),
    );
    let fixed = by_tests(&mut service)
        .add_fixed_run(weekly("222", Weekday::Mon, hm(21, 30), &["1001", "1002"]))
        .await
        .unwrap();
    by_tests(&mut service)
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let run_at = |at: DateTime<FixedOffset>| {
        let run = state.runs.iter().find(|run| run.datetime == utc(at));
        run.unwrap().id.clone()
    };
    let runs = [
        run_at(kl(31, 21, 30)),
        run_at(kl(7, 21, 30)),
        run_at(kl(14, 21, 30)),
    ];
    by_tests(&mut service)
        .amend_run(&runs[1], utc(kl(9, 21, 0)), &policy())
        .await
        .unwrap();
    OneTiming {
        service,
        clock,
        fixed,
        runs,
    }
}

/// Two weekly timings (Mon 21:30 in 222, Tue 21:30 in 333; party 1001 and
/// 1002) with runs for the weeks of 27 Aug, 3 Sep and 10 Sep; each timing's
/// middle run is amended a day later. Random ids like production: replay
/// draws must never collide with stored rows, or the owner log renames the
/// wrong rows.
pub struct TwoTimings {
    pub service: SchedulerService<SqliteStore, RandomIds, TestClock>,
    pub fixed: [String; 2],
    pub runs: [[String; 3]; 2],
}

pub async fn two_timings() -> TwoTimings {
    let mut service = SchedulerService::new(
        SqliteStore::open_temp().await,
        RandomIds,
        TestClock::new(kl(27, 1, 0)),
    );
    let mut fixed = Vec::new();
    for (weekday, channel) in [(Weekday::Mon, "222"), (Weekday::Tue, "333")] {
        let timing = weekly(channel, weekday, hm(21, 30), &["1001", "1002"]);
        let id = by_tests(&mut service).add_fixed_run(timing);
        fixed.push(id.await.unwrap());
    }
    by_tests(&mut service)
        .materialise_weeks(&policy())
        .await
        .unwrap();
    let state = snapshot(&service).await;
    let runs = [0, 1].map(|index| {
        let mut rows: Vec<_> = state
            .runs
            .iter()
            .filter(|run| run.fixed_run_id.as_deref() == Some(fixed[index].as_str()))
            .collect();
        rows.sort_by_key(|run| run.datetime);
        [0, 1, 2].map(|week| rows[week].id.clone())
    });
    for ids in &runs {
        let to = state
            .runs
            .iter()
            .find(|run| run.id == ids[1])
            .unwrap()
            .datetime;
        by_tests(&mut service)
            .amend_run(&ids[1], to + TimeDelta::days(1), &policy())
            .await
            .unwrap();
    }
    TwoTimings {
        service,
        fixed: [fixed[0].clone(), fixed[1].clone()],
        runs,
    }
}
