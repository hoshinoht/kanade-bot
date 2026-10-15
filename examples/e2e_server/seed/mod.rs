//! What the template store holds: the retired dev mock's seeded guild (last
//! in the tree at commit 05bda3d0) with the mock's ids, names and times,
//! written through the real store APIs, plus the fake Discord sign-in users
//! `e2e/harness/member.spec` reads. Written over the composed state, so every
//! later generation (composed afresh from a copy) reads it as a restarted
//! server would.
//!
//! Times are relative to this boss week, so the seed reads the same at the
//! pinned instant (`2026-09-29T04:00:00Z`, Tue 12:00 in Kuala Lumpur) and
//! on a live clock: [`Seed::now`] is as far into the current boss week as
//! the pinned instant is into its own.

pub mod auth_extra;
mod backups;
mod characters;
pub mod ids;
mod inbox;
mod logs;
mod members;
mod notify;
mod p5;
pub mod p6;
mod schedule;
mod sessions;
mod settings;

use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveTime, Utc};
use kanade::{
    api::state::ApiState,
    domain::{
        history::{Actor, ChangeHistory, ChangeMeta, ChangeRef, Origin, Surface},
        ids::{IdGenerator, RandomIds},
        members::{MemberStore, Roster},
        schedule::{Change, ChangeSet, utc_instant},
        scheduler::{Clock, ScheduleStore, SchedulerService, Scope},
    },
    infrastructure::store::SqliteStore,
};

use crate::Fail;

pub use members::{
    BOSSING_ROLE, CHANNELS, PILOT_ROLE, ROLES, STAFF_ROLE, member_name, watched_ids,
};
pub use settings::persona_files;

/// How far into its boss week (Thu 00:00 KL) the pinned instant is.
const INTO_WEEK: Duration = Duration::hours(5 * 24 + 12);

/// Discord's epoch in Unix milliseconds.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// The scheduler's clock standing at one seeded instant.
struct At(DateTime<Utc>);

impl Clock for At {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// The ids a seeded write gives its new rows, in order (the mock's ids);
/// random ones past the list.
struct Ids(std::vec::IntoIter<String>);

impl IdGenerator for Ids {
    fn new_id(&mut self) -> String {
        self.0.next().unwrap_or_else(|| RandomIds.new_id())
    }
}

type Scheduler = SchedulerService<Arc<SqliteStore>, Ids, At>;

pub struct Seed<'a> {
    store: Arc<SqliteStore>,
    state: &'a ApiState,
    /// This boss week's start.
    week: DateTime<Utc>,
    now: DateTime<Utc>,
}

/// `HH:MM`.
fn hhmm(text: &str) -> Result<NaiveTime, Fail> {
    NaiveTime::parse_from_str(text, "%H:%M").map_err(|_| format!("not HH:MM: {text}").into())
}

/// Minutes past midnight of `HH:MM`.
fn minute_of(text: &str) -> Result<i64, Fail> {
    Ok(hhmm(text)?
        .signed_duration_since(NaiveTime::MIN)
        .num_minutes())
}

impl Seed<'_> {
    /// A guild-local wall clock on `day` (0 = reset day) of this boss week
    /// (`week` 0), last week (-1) or next week (1); midnight without `time`.
    /// The guild's zone has no daylight saving, so local days are 24 h.
    fn at(&self, week: i64, day: i64, time: Option<&str>) -> Result<DateTime<Utc>, Fail> {
        let minutes = time.map(minute_of).transpose()?.unwrap_or(0);
        Ok(self.week + Duration::days(week * 7 + day) + Duration::minutes(minutes))
    }

    /// `seconds` before the seed's now.
    fn ago(&self, seconds: i64) -> DateTime<Utc> {
        self.now - Duration::seconds(seconds)
    }

    /// `hours` past 8 h before this boss week's start: the clock the mock's
    /// logs and inbox count in (`Store::hour_minute`).
    fn hour(&self, hours: i64) -> DateTime<Utc> {
        self.week + Duration::hours(hours - 8)
    }

    /// A Discord snowflake for a message posted at `at`; `n` keeps messages
    /// of the same millisecond apart.
    fn snowflake(at: DateTime<Utc>, n: u64) -> String {
        let since = u64::try_from(at.timestamp_millis() - DISCORD_EPOCH_MS).unwrap_or(0);
        ((since << 22) | (n & 0x3f_ffff)).to_string()
    }

    /// The scheduler as the API's writer composes it, on a clock standing at
    /// `at`, giving new rows `ids`.
    fn scheduler(&self, at: DateTime<Utc>, ids: &[&str]) -> Scheduler {
        let ids: Vec<String> = ids.iter().map(|id| (*id).to_owned()).collect();
        SchedulerService::new(self.store.clone(), Ids(ids.into_iter()), At(at))
            .with_run_ends(self.state.run_ends_source())
    }

    /// What writes validate against: the members and the watched channels.
    async fn directory(&self) -> Result<Roster, Fail> {
        let mut roster = Roster::new();
        for profile in self.store.list_members().await? {
            roster.upsert(profile.member);
        }
        for channel in CHANNELS.iter().filter(|channel| channel.watched) {
            roster.watch(channel.id);
        }
        Ok(roster)
    }

    /// Commit `changes` as `actor` on `surface` at `at`; the new head.
    async fn commit(
        &self,
        actor: Actor,
        surface: Surface,
        at: DateTime<Utc>,
        changes: Vec<Change>,
        refs: Vec<ChangeRef>,
    ) -> Result<ChangeRef, Fail> {
        let revision = self.store.load(&Scope::All).await?.revision;
        let meta = ChangeMeta {
            origin: Origin::new(actor, surface),
            at,
            notices: Vec::new(),
            refs,
            request_digest: None,
            expect: Default::default(),
            outbox: Vec::new(),
        };
        self.store
            .commit(revision, ChangeSet { changes }, meta)
            .await?
            .ok_or("a seed commit changed nothing")?;
        Ok(self.store.history_head().await?)
    }
}

pub async fn template(store: Arc<SqliteStore>, state: &ApiState) -> Result<(), Fail> {
    let now = (state.clock)();
    let week = utc_instant(&state.policy.week_of(&now)?)?;
    let seed = Seed {
        store,
        state,
        week,
        now: week + INTO_WEEK,
    };
    settings::seed(&seed).await?;
    members::seed(&seed).await?;
    schedule::seed(&seed).await?;
    characters::seed(&seed).await?;
    p5::seed(&seed).await?;
    logs::seed(&seed).await?;
    inbox::seed(&seed).await?;
    sessions::seed(&seed).await?;
    notify::seed(&seed).await?;
    // Last: the manifests name the finished seed's history head.
    backups::seed(&seed).await?;
    Ok(())
}
