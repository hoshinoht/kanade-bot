//! `week.json`: the admin week board, stats and summary.

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;

use super::{Art, Boss, Named, bosses, dow, hhmm, iso_date, iso_instant};
use crate::{
    api::state::ChannelEntry,
    domain::{
        catalog::BossTable,
        ids::short_id,
        members::{Roster, member_name},
        schedule::{
            DAY_OF, FixedRun, Reminder, RsvpState, Run, RunSource, RunStatus, ScheduleSnapshot,
            countdown_kind, is_stale,
        },
    },
};

/// What every projection reads besides the rows themselves.
pub struct Context<'a> {
    pub catalog: &'a BossTable,
    pub art: Art<'a>,
    pub zone: Tz,
    pub now: DateTime<Utc>,
    pub roster: Roster,
    pub channels: BTreeMap<String, ChannelEntry>,
    pub guild_id: Option<&'a str>,
}

impl Context<'_> {
    pub fn name(&self, user_id: &str) -> String {
        member_name(&self.roster, user_id)
    }

    pub fn named(&self, user_id: &str) -> Named {
        Named {
            id: user_id.to_owned(),
            name: self.name(user_id),
        }
    }

    pub fn channel_name(&self, id: &str) -> String {
        self.channels
            .get(id)
            .map_or_else(|| id.to_owned(), |channel| channel.name.clone())
    }

    pub fn local_date(&self, at: DateTime<Utc>) -> NaiveDate {
        self.zone.from_utc_datetime(&at.naive_utc()).date_naive()
    }

    pub fn bosses(&self, tokens: &[String]) -> Vec<Boss> {
        bosses(self.catalog, &self.art, tokens)
    }
}

#[derive(Serialize)]
pub struct WeekDay {
    pub index: u8,
    pub date: String,
    pub dow: &'static str,
    pub is_reset: bool,
    pub is_today: bool,
}

#[derive(Serialize)]
pub struct Tally {
    pub on: usize,
    pub total: usize,
}

#[derive(Serialize)]
pub struct Participant {
    pub id: String,
    pub name: String,
    pub answer: &'static str,
}

#[derive(Serialize)]
pub struct ReminderCard {
    pub label: &'static str,
    pub state: &'static str,
    pub at: String,
    pub url: Option<String>,
}

#[derive(Serialize)]
pub struct RosterChange {
    pub out: Vec<Named>,
    #[serde(rename = "in")]
    pub added: Vec<Named>,
}

#[derive(Serialize)]
pub struct RunDto {
    pub id: String,
    pub day: u8,
    pub time: Option<String>,
    pub status: &'static str,
    pub bosses: Vec<Boss>,
    pub tally: Tally,
    pub short_id: String,
    pub participants: Vec<Participant>,
    pub party: String,
    pub channel_id: String,
    pub channel: String,
    pub cards: Vec<ReminderCard>,
    pub fixed_id: Option<String>,
    pub amended: bool,
    pub roster_change: Option<RosterChange>,
}

#[derive(Serialize)]
pub struct Week {
    pub starts: String,
    pub timezone: String,
    pub reset: String,
    pub days: Vec<WeekDay>,
    pub runs: Vec<RunDto>,
    pub generated_at: String,
    pub version: u64,
}

/// The card label the PWA knows; other countdown lengths have no label yet.
pub fn card_label(kind: &str) -> Option<&'static str> {
    if kind == DAY_OF {
        Some("morning")
    } else if kind == countdown_kind(60) {
        Some("T-1h")
    } else if kind == countdown_kind(15) {
        Some("T-15m")
    } else {
        None
    }
}

/// `posted` (sent with a message), `skipped` (retired unsent, or too late to
/// post) or `queued`.
pub fn card_state(
    reminder: &Reminder,
    snapshot: &ScheduleSnapshot,
    now: DateTime<Utc>,
) -> &'static str {
    let unproven = snapshot.unproven_retired.contains(&reminder.id);
    match (reminder.sent_at, &reminder.message_id) {
        (Some(_), Some(_)) if !unproven => "posted",
        (Some(_), _) => "skipped",
        (None, _) if unproven || is_stale(&reminder.kind, reminder.fire_at, now) => "skipped",
        (None, _) => "queued",
    }
}

pub fn message_url(ctx: &Context<'_>, run: &Run, reminder: &Reminder) -> Option<String> {
    Some(format!(
        "https://discord.com/channels/{}/{}/{}",
        ctx.guild_id?,
        run.channel_id.as_deref()?,
        reminder.message_id.as_deref()?
    ))
}

pub fn answers(snapshot: &ScheduleSnapshot, run: &Run) -> BTreeMap<String, RsvpState> {
    snapshot
        .rsvps
        .iter()
        .filter(|rsvp| rsvp.run_id == run.id)
        .map(|rsvp| (rsvp.user_id.clone(), rsvp.state))
        .collect()
}

fn answer(state: Option<&RsvpState>) -> &'static str {
    state.map_or("waiting", |state| state.as_str())
}

/// Day index within the boss week (0 = reset day).
pub fn day_index(ctx: &Context<'_>, start_date: NaiveDate, at: DateTime<Utc>) -> u8 {
    (ctx.local_date(at) - start_date).num_days().clamp(0, 6) as u8
}

/// Own-time runs have no fixed start on the board.
pub fn run_time(ctx: &Context<'_>, run: &Run) -> Option<String> {
    (run.status != RunStatus::Otot)
        .then(|| hhmm(ctx.zone.from_utc_datetime(&run.datetime.naive_utc())))
}

fn roster_change(ctx: &Context<'_>, run: &Run, fixed: Option<&FixedRun>) -> Option<RosterChange> {
    let fixed = fixed?;
    let change = RosterChange {
        out: fixed
            .participants
            .iter()
            .filter(|id| !run.participants.contains(id))
            .map(|id| ctx.named(id))
            .collect(),
        added: run
            .participants
            .iter()
            .filter(|id| !fixed.participants.contains(id))
            .map(|id| ctx.named(id))
            .collect(),
    };
    (!change.out.is_empty() || !change.added.is_empty()).then_some(change)
}

pub fn run_dto(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    start_date: NaiveDate,
    run: &Run,
) -> RunDto {
    let answers = answers(snapshot, run);
    let participants: Vec<Participant> = run
        .participants
        .iter()
        .map(|id| Participant {
            id: id.clone(),
            name: ctx.name(id),
            answer: answer(answers.get(id)),
        })
        .collect();
    let fixed = run
        .fixed_run_id
        .as_deref()
        .and_then(|id| snapshot.fixed_runs.iter().find(|fixed| fixed.id == id));
    let channel_id = run.channel_id.clone().unwrap_or_default();
    let cards = snapshot
        .reminders
        .iter()
        .filter(|reminder| reminder.run_id == run.id)
        .filter_map(|reminder| {
            let label = card_label(&reminder.kind)?;
            let state = card_state(reminder, snapshot, ctx.now);
            Some((
                reminder.fire_at,
                ReminderCard {
                    label,
                    state,
                    at: hhmm(ctx.zone.from_utc_datetime(&reminder.fire_at.naive_utc())),
                    url: (state == "posted")
                        .then(|| message_url(ctx, run, reminder))
                        .flatten(),
                },
            ))
        });
    let mut cards: Vec<_> = cards.collect();
    cards.sort_by_key(|(at, _)| *at);
    RunDto {
        id: run.id.clone(),
        day: day_index(ctx, start_date, run.datetime),
        time: run_time(ctx, run),
        status: run.status.as_str(),
        bosses: ctx.bosses(&run.bosses),
        tally: Tally {
            on: participants.iter().filter(|p| p.answer == "yes").count(),
            total: participants.len(),
        },
        short_id: short_id(&run.id),
        participants,
        party: ctx.channel_name(&channel_id),
        channel: ctx.channel_name(&channel_id),
        channel_id,
        cards: cards.into_iter().map(|(_, card)| card).collect(),
        fixed_id: run.fixed_run_id.clone(),
        amended: run.source == RunSource::Amend,
        roster_change: roster_change(ctx, run, fixed),
    }
}

/// The boss week starting at `start` (a UTC instant of the reset).
pub struct WeekFrame {
    pub start: DateTime<Utc>,
    pub reset: String,
}

pub fn days(ctx: &Context<'_>, start_date: NaiveDate) -> Vec<WeekDay> {
    let today = ctx.local_date(ctx.now);
    (0..7u8)
        .map(|index| {
            let date = start_date + chrono::Days::new(u64::from(index));
            WeekDay {
                index,
                date: iso_date(date),
                dow: dow(date.weekday()),
                is_reset: index == 0,
                is_today: date == today,
            }
        })
        .collect()
}

pub fn week(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    frame: &WeekFrame,
    version: u64,
) -> Week {
    let start_date = ctx.local_date(frame.start);
    Week {
        starts: iso_date(start_date),
        timezone: ctx.zone.name().to_owned(),
        reset: frame.reset.clone(),
        days: days(ctx, start_date),
        runs: snapshot
            .runs
            .iter()
            .filter(|run| run.week_start == frame.start)
            .map(|run| run_dto(ctx, snapshot, start_date, run))
            .collect(),
        generated_at: iso_instant(ctx.now),
        version,
    }
}

#[derive(Serialize)]
pub struct DayStat {
    pub day: u8,
    pub answered: usize,
    pub waiting: usize,
}

#[derive(Serialize)]
pub struct Stats {
    pub per_day: Vec<DayStat>,
}

pub fn stats(ctx: &Context<'_>, snapshot: &ScheduleSnapshot, frame: &WeekFrame) -> Stats {
    let start_date = ctx.local_date(frame.start);
    let mut per_day: Vec<DayStat> = (0..7u8)
        .map(|day| DayStat {
            day,
            answered: 0,
            waiting: 0,
        })
        .collect();
    for run in snapshot
        .runs
        .iter()
        .filter(|run| run.week_start == frame.start)
    {
        let answers = answers(snapshot, run);
        let stat = &mut per_day[usize::from(day_index(ctx, start_date, run.datetime))];
        for id in &run.participants {
            if answers.contains_key(id) {
                stat.answered += 1;
            } else {
                stat.waiting += 1;
            }
        }
    }
    Stats { per_day }
}

#[derive(Serialize)]
pub struct NextRun {
    pub run_id: String,
    pub bosses: String,
    pub when: String,
    pub countdown: String,
    pub on: usize,
    pub total: usize,
}

#[derive(Serialize)]
pub struct Model {
    pub busy: bool,
    pub holder: Option<String>,
}

#[derive(Serialize)]
pub struct Summary {
    pub next: Option<NextRun>,
    pub unanswered: usize,
    pub inbox: u64,
    pub model: Model,
}

fn is_ahead(run: &Run, now: DateTime<Utc>) -> bool {
    matches!(
        run.status,
        RunStatus::Planned | RunStatus::Confirmed | RunStatus::AtRisk
    ) && run.datetime > now
}

pub fn summary(ctx: &Context<'_>, snapshot: &ScheduleSnapshot, inbox: u64) -> Summary {
    let next = snapshot
        .runs
        .iter()
        .filter(|run| is_ahead(run, ctx.now))
        .min_by_key(|run| (run.datetime, &run.id))
        .map(|run| {
            let answers = answers(snapshot, run);
            NextRun {
                run_id: run.id.clone(),
                bosses: run.bosses.join(" + "),
                when: super::when(run.datetime, ctx.zone),
                countdown: super::countdown((run.datetime - ctx.now).num_minutes()),
                on: run
                    .participants
                    .iter()
                    .filter(|id| answers.get(*id) == Some(&RsvpState::Yes))
                    .count(),
                total: run.participants.len(),
            }
        });
    let unanswered = snapshot
        .runs
        .iter()
        .filter(|run| is_ahead(run, ctx.now))
        .map(|run| {
            let answers = answers(snapshot, run);
            run.participants
                .iter()
                .filter(|id| !answers.contains_key(*id))
                .count()
        })
        .sum();
    Summary {
        next,
        unanswered,
        inbox,
        // The model governor is not composed into the API yet.
        model: Model {
            busy: false,
            holder: None,
        },
    }
}
