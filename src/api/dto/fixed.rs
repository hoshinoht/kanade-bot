//! `fixed.json`: weekly timings with their live runs this and next week.

use chrono::{DateTime, Utc};
use serde::Serialize;

use super::{
    Boss, Named, hhmm,
    week::{Context, day_index, is_amended, run_time},
    weekday_name,
};
use crate::domain::{
    ids::short_id,
    schedule::{FixedRun, ScheduleSnapshot},
};

#[derive(Serialize)]
pub struct FixedRunLink {
    pub run_id: String,
    pub short_id: String,
    pub week: &'static str,
    pub day: u8,
    pub time: Option<String>,
    pub status: &'static str,
    pub amended: bool,
}

#[derive(Serialize)]
pub struct FixedRow {
    pub id: String,
    pub short_id: String,
    pub weekday: u32,
    pub weekday_name: &'static str,
    pub time: String,
    pub bosses: Vec<Boss>,
    pub participants: Vec<Named>,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_watched: bool,
    pub owner: String,
    pub note: Option<String>,
    pub runs: Vec<FixedRunLink>,
}

/// `weeks`: this and next boss week starts, in that order.
pub fn rows(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    weeks: [DateTime<Utc>; 2],
) -> Vec<FixedRow> {
    snapshot
        .fixed_runs
        .iter()
        .map(|fixed| row(ctx, snapshot, weeks, fixed))
        .collect()
}

fn row(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    weeks: [DateTime<Utc>; 2],
    fixed: &FixedRun,
) -> FixedRow {
    let channel_id = fixed.channel_id.clone().unwrap_or_default();
    let runs = snapshot
        .runs
        .iter()
        .filter(|run| {
            run.fixed_run_id.as_deref() == Some(fixed.id.as_str()) && run.status.is_live()
        })
        .filter_map(|run| {
            let week = match weeks.iter().position(|start| *start == run.week_start)? {
                0 => "this",
                _ => "next",
            };
            let start_date = ctx.local_date(run.week_start);
            Some(FixedRunLink {
                run_id: run.id.clone(),
                short_id: short_id(&run.id),
                week,
                day: day_index(ctx, start_date, run.datetime),
                time: run_time(ctx, run),
                status: run.status.as_str(),
                amended: is_amended(ctx, run, Some(fixed)),
            })
        })
        .collect();
    FixedRow {
        id: fixed.id.clone(),
        short_id: short_id(&fixed.id),
        weekday: fixed.weekday.num_days_from_monday(),
        weekday_name: weekday_name(fixed.weekday),
        time: hhmm(fixed.time),
        bosses: ctx.bosses(&fixed.bosses),
        participants: fixed.participants.iter().map(|id| ctx.named(id)).collect(),
        channel_name: ctx.channel_name(&channel_id),
        channel_watched: ctx
            .channels
            .get(&channel_id)
            .is_some_and(|channel| channel.watched),
        channel_id,
        owner: ctx.name(&fixed.owner_id),
        note: fixed.note.clone(),
        runs,
    }
}
