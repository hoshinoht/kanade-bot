//! The background estimate refresh's view of the schedule: the main
//! store's current and next boss weeks (from the reset saved now), each run
//! in the refresh horizon with its party resolved as "Look up party" and
//! the reminder chart resolve it.

use std::{
    collections::{BTreeMap, HashMap, hash_map::Entry},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use tokio::sync::watch;

use crate::{
    api::{admin::config::SettingsChanged, state::ReadStore},
    infrastructure::{
        scouter::{
            Upcoming, UpcomingFuture, UpcomingParties, UpcomingRun, party_subjects,
            refresh::in_horizon,
        },
        store::SqliteStore,
    },
};
use kanade_domain::{
    catalog::BossTable,
    characters::PartyRoster,
    schedule::{Run, utc_instant},
    scheduler::Scope,
};

pub(super) struct StoreParties {
    store: Arc<SqliteStore>,
    catalog: Arc<BossTable>,
    settings: watch::Receiver<SettingsChanged>,
    zone: Tz,
}

impl StoreParties {
    pub(super) fn new(
        store: Arc<SqliteStore>,
        catalog: Arc<BossTable>,
        settings: watch::Receiver<SettingsChanged>,
        zone: Tz,
    ) -> Self {
        Self {
            store,
            catalog,
            settings,
            zone,
        }
    }
}

impl UpcomingParties for StoreParties {
    fn upcoming(&self, now: DateTime<Utc>) -> UpcomingFuture<'_> {
        Box::pin(async move {
            let policy = self.settings.borrow().settings.schedule_policy(self.zone);
            let [current, next, _] = policy.materialised_weeks(now).ok()?;
            let (current, next) = (utc_instant(&current).ok()?, utc_instant(&next).ok()?);
            let snapshot =
                ReadStore::snapshot(self.store.as_ref(), Scope::Weeks(vec![current, next]))
                    .await
                    .ok()?;
            let mut weeks: BTreeMap<DateTime<Utc>, Vec<Run>> = BTreeMap::new();
            for run in &snapshot.runs {
                weeks.entry(run.week_start).or_default().push(run.clone());
            }
            // A roster depends only on the run's participants and its week.
            let mut rosters: HashMap<(DateTime<Utc>, &[String]), PartyRoster> = HashMap::new();
            let mut runs = Vec::new();
            for run in snapshot
                .runs
                .iter()
                .filter(|run| in_horizon(run.status, run.datetime, run.week_start, current, now))
            {
                let week = weeks[&run.week_start].as_slice();
                let roster = match rosters.entry((run.week_start, run.participants.as_slice())) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => entry.insert(
                        PartyRoster::load(self.store.as_ref(), run, week)
                            .await
                            .ok()?,
                    ),
                };
                let slots = roster.slots(run, week, &snapshot.fixed_runs, &self.catalog);
                let (subjects, missing, parties) = party_subjects(&slots, &run.bosses);
                runs.push(UpcomingRun {
                    run_id: run.id.clone(),
                    bosses: run.bosses.clone(),
                    status: run.status,
                    datetime: run.datetime,
                    week_start: run.week_start,
                    subjects,
                    missing,
                    parties,
                });
            }
            Some(Upcoming {
                current_week: current,
                runs,
            })
        })
    }
}
