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
            DAY_OF, FixedRun, Reminder, RsvpState, Run, RunStatus, ScheduleSnapshot,
            countdown_kind, is_stale,
        },
        settings::RunLengths,
    },
    infrastructure::llm::governor::GroupSnapshot,
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
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct WeekDay {
    pub index: u8,
    pub date: String,
    pub dow: &'static str,
    pub is_reset: bool,
    pub is_today: bool,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Tally {
    pub on: usize,
    pub total: usize,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Participant {
    pub id: String,
    pub name: String,
    #[cfg_attr(test, ts(type = "Answer"))]
    pub answer: &'static str,
}

/// The reminder cards the PWA labels: the day-of card and two countdowns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum CardKind {
    #[serde(rename = "morning")]
    Morning,
    #[serde(rename = "T-1h")]
    HourBefore,
    #[serde(rename = "T-15m")]
    QuarterBefore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    Posted,
    Queued,
    Skipped,
}

/// This or next boss week.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WeekKey {
    This,
    Next,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct ReminderCard {
    pub label: CardKind,
    pub state: CardState,
    pub at: String,
    pub url: Option<String>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct RosterChange {
    pub out: Vec<Named>,
    #[serde(rename = "in")]
    pub added: Vec<Named>,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "Run"))]
pub struct RunDto {
    pub id: String,
    pub day: u8,
    pub time: Option<String>,
    /// Derived from every boss even when an own-time run has no start clock.
    pub minutes: u32,
    #[cfg_attr(test, ts(type = "RunStatus"))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
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
pub fn card_label(kind: &str) -> Option<CardKind> {
    if kind == DAY_OF {
        Some(CardKind::Morning)
    } else if kind == countdown_kind(60) {
        Some(CardKind::HourBefore)
    } else if kind == countdown_kind(15) {
        Some(CardKind::QuarterBefore)
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
) -> CardState {
    let unproven = snapshot.unproven_retired.contains(&reminder.id);
    match (reminder.sent_at, &reminder.message_id) {
        (Some(_), Some(_)) if !unproven => CardState::Posted,
        (Some(_), _) => CardState::Skipped,
        (None, _) if unproven || is_stale(&reminder.kind, reminder.fire_at, now) => {
            CardState::Skipped
        }
        (None, _) => CardState::Queued,
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

/// Everyone on the run with their name and answer (`waiting` when none).
pub fn participants(ctx: &Context<'_>, snapshot: &ScheduleSnapshot, run: &Run) -> Vec<Participant> {
    let answers = answers(snapshot, run);
    run.participants
        .iter()
        .map(|id| Participant {
            id: id.clone(),
            name: ctx.name(id),
            answer: answer(answers.get(id)),
        })
        .collect()
}

pub fn tally(participants: &[Participant]) -> Tally {
    Tally {
        on: participants.iter().filter(|p| p.answer == "yes").count(),
        total: participants.len(),
    }
}

pub fn run_dto(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    start_date: NaiveDate,
    run: &Run,
    run_lengths: &RunLengths,
) -> RunDto {
    let participants = participants(ctx, snapshot, run);
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
                    url: (state == CardState::Posted)
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
        minutes: run_lengths.minutes_for(ctx.catalog, &run.bosses),
        status: run.status.as_str(),
        bosses: ctx.bosses(&run.bosses),
        tally: tally(&participants),
        short_id: short_id(&run.id),
        participants,
        party: ctx.channel_name(&channel_id),
        channel: ctx.channel_name(&channel_id),
        channel_id,
        cards: cards.into_iter().map(|(_, card)| card).collect(),
        fixed_id: run.fixed_run_id.clone(),
        amended: is_amended(ctx, run, fixed),
        roster_change: roster_change(ctx, run, fixed),
    }
}

/// Off its weekly timing this week: another day or time, or another roster.
/// Derived rather than read from `source`, which stays `amend` after a run is
/// reset back onto its timing.
pub fn is_amended(ctx: &Context<'_>, run: &Run, fixed: Option<&FixedRun>) -> bool {
    let Some(fixed) = fixed else {
        return false;
    };
    let local = ctx.zone.from_utc_datetime(&run.datetime.naive_utc());
    let moved = local.weekday() != fixed.weekday
        || (run.status != RunStatus::Otot && local.time() != fixed.time);
    moved || roster_change(ctx, run, Some(fixed)).is_some()
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
    run_lengths: &RunLengths,
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
            .map(|run| run_dto(ctx, snapshot, start_date, run, run_lengths))
            .collect(),
        generated_at: iso_instant(ctx.now),
        version,
    }
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct DayStat {
    pub day: u8,
    pub answered: usize,
    pub waiting: usize,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
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
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct NextRun {
    pub run_id: String,
    pub bosses: String,
    pub when: String,
    pub countdown: String,
    pub on: usize,
    pub total: usize,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS), ts(rename = "ModelBusy"))]
pub struct Model {
    pub busy: bool,
    pub holder: Option<String>,
}

impl Model {
    /// Busy when a governor group has every permit in use; the holder is
    /// that group's longest-held permit's call kind (groups in snapshot
    /// order). Holders are listed by permit sequence, but grants go by
    /// priority first, so the oldest is the largest `held_s`; a tie goes to
    /// the lower sequence (listed first).
    pub fn from_groups(groups: &[GroupSnapshot]) -> Self {
        let full = groups
            .iter()
            .find(|group| group.permits.total > 0 && group.permits.in_use >= group.permits.total);
        Self {
            busy: full.is_some(),
            holder: full
                .and_then(|group| {
                    group
                        .holders
                        .iter()
                        .enumerate()
                        .max_by_key(|(index, holder)| (holder.held_s, std::cmp::Reverse(*index)))
                })
                .map(|(_, holder)| holder)
                .map(|holder| holder.kind.as_str().to_owned()),
        }
    }
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Summary {
    pub next: Option<NextRun>,
    pub unanswered: usize,
    pub inbox: u64,
    /// Listed members with the bossing role, as `/api/admin/members` counts them.
    pub members: usize,
    /// `/api/admin/reminders` `upcoming` rows over the same two weeks.
    pub reminders: usize,
    pub model: Model,
    /// Notifications `quiet_mode` as the running settings hold it (the shell's chip).
    pub quiet_mode: bool,
    /// Why Re-read would be refused right now: extraction switched off (the
    /// `409 extraction_off` sentence) or no extractor composed (the `503
    /// unavailable` sentence); `null` while re-reading can run.
    pub rescan_off: Option<String>,
}

/// The summary's live, non-schedule facts.
pub struct Live {
    pub quiet_mode: bool,
    pub model: Model,
    pub rescan_off: Option<String>,
}

fn is_ahead(run: &Run, now: DateTime<Utc>) -> bool {
    matches!(
        run.status,
        RunStatus::Planned | RunStatus::Confirmed | RunStatus::AtRisk
    ) && run.datetime > now
}

/// The next live run after `now`: the summary's `next` and the sign-in strip.
fn next_run(snapshot: &ScheduleSnapshot, now: DateTime<Utc>) -> Option<&Run> {
    snapshot
        .runs
        .iter()
        .filter(|run| is_ahead(run, now))
        .min_by_key(|run| (run.datetime, &run.id))
}

fn yes_count(snapshot: &ScheduleSnapshot, run: &Run) -> usize {
    let answers = answers(snapshot, run);
    run.participants
        .iter()
        .filter(|id| answers.get(*id) == Some(&RsvpState::Yes))
        .count()
}

pub fn summary(
    ctx: &Context<'_>,
    snapshot: &ScheduleSnapshot,
    inbox: u64,
    members: usize,
    live: Live,
) -> Summary {
    let next = next_run(snapshot, ctx.now).map(|run| NextRun {
        run_id: run.id.clone(),
        bosses: run.bosses.join(" + "),
        when: super::when(run.datetime, ctx.zone),
        countdown: super::countdown((run.datetime - ctx.now).num_minutes()),
        on: yes_count(snapshot, run),
        total: run.participants.len(),
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
        members,
        reminders: super::reminders::upcoming(ctx, snapshot),
        model: live.model,
        quiet_mode: live.quiet_mode,
        rescan_off: live.rescan_off,
    }
}

/// The signed-out sign-in strip: no names, ids, answers, party, channel or version.
#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct TonightRun {
    /// Guild-local `HH:MM`.
    pub time: String,
    /// Catalog display names.
    pub bosses: Vec<String>,
    /// The public week's aggregate (`on`/`total`).
    pub tally: Tally,
}

#[derive(Serialize)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct Tonight {
    /// The next live run when it starts later today in the guild zone, else `null`.
    pub run: Option<TonightRun>,
}

pub fn tonight(ctx: &Context<'_>, snapshot: &ScheduleSnapshot) -> Tonight {
    let run = next_run(snapshot, ctx.now)
        .filter(|run| ctx.local_date(run.datetime) == ctx.local_date(ctx.now))
        .map(|run| TonightRun {
            time: hhmm(ctx.zone.from_utc_datetime(&run.datetime.naive_utc())),
            bosses: ctx
                .bosses(&run.bosses)
                .into_iter()
                .map(|boss| boss.name)
                .collect(),
            tally: Tally {
                on: yes_count(snapshot, run),
                total: run.participants.len(),
            },
        });
    Tonight { run }
}
