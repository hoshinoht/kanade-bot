//! What the template store holds: the dev mock's seeded guild
//! (`devtools/pwa-mock/src/mock/seed.rs` and the seeded state of its domain
//! modules) with the mock's ids, names and times, written through the real
//! store APIs, plus the fake Discord sign-in users `e2e/harness/member.spec`
//! reads. Written over the composed state, so every later generation
//! (composed afresh from a copy) reads it as a restarted server would.
//!
//! Times are relative to this boss week, so the seed reads the same at the
//! pinned instant (`2026-09-29T04:00:00Z`, Tue 12:00 in Kuala Lumpur) and
//! on a live clock: [`Seed::now`] is as far into the current boss week as
//! the pinned instant is into its own.

mod characters;
mod members;
mod schedule;
mod settings;

use std::sync::Arc;

use chrono::{DateTime, Duration, NaiveTime, Utc};
use kanade::{
    api::state::ApiState,
    domain::{
        history::{Actor, ChangeHistory, ChangeMeta, ChangeRef, Origin, Surface},
        schedule::{Change, ChangeSet, utc_instant},
        scheduler::{ScheduleStore, Scope},
    },
    infrastructure::store::SqliteStore,
};

use crate::Fail;

pub use members::{BOSSING_ROLE, CHANNELS, PILOT_ROLE, ROLES, STAFF_ROLE, member_name};
pub use settings::persona_files;

/// How far into its boss week (Thu 00:00 KL) the pinned instant is.
const INTO_WEEK: Duration = Duration::hours(5 * 24 + 12);

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
    Ok(())
}
