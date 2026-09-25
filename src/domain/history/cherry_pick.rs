//! Cherry-pick: re-apply one recorded change of a weekly timing's run to the
//! same timing's run in another boss week. Pure planning: the scheduler
//! service replays the planned steps through the normal mutation path and
//! records one `Surface::CherryPick` change referencing the picked record.
//!
//! Supported: a run's slot (same local weekday and time in the target week,
//! placed DST-aware by `slot_in_week`), its party (as a delta), its status
//! (when it is the only change), and answers of members on the target run.
//! Everything else is `UnsupportedPick`. A field the target already holds
//! at its picked value is skipped (no step, no conflict).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, Datelike, NaiveTime, Utc, Weekday};

use super::origin::Surface;
use super::precondition::Expect;
use super::record::{ChangeRecord, RowKey, RowValue};
use crate::domain::schedule::{
    FixedRun, RsvpSource, RsvpState, Run, RunStatus, SchedulePolicy, ScheduleSnapshot, utc_instant,
};
use crate::domain::time::{AwareDateTime, ZonedDateTime};
use crate::domain::weeks::slot_in_week;

/// Strict refuses a pick whose target no longer holds the picked change's
/// `before` values. Force applies it anyway, bound to the conflicts the
/// administrator reviewed: exactly the preview's `force` expectations
/// (conflicting fields with the change that last set each, and those
/// changes as overrides), which the store checks inside the commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickMode {
    Strict,
    Force(Expect),
}

/// One planned mutation on a target run, in apply order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickStep {
    Amend {
        run_id: String,
        to: DateTime<Utc>,
    },
    Swap {
        run_id: String,
        remove: Vec<String>,
        add: Vec<String>,
    },
    Status {
        run_id: String,
        status: RunStatus,
    },
    /// Followed by a recount of the run's status, as a reaction does.
    Rsvp {
        run_id: String,
        user_id: String,
        state: RsvpState,
        source: RsvpSource,
    },
}

/// A field value compared by the precondition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickValue {
    Slot(DateTime<Utc>),
    /// Sorted.
    Party(Vec<String>),
    Status(RunStatus),
    Answer(Option<RsvpState>),
}

/// The target no longer holds the picked change's `before` value (mapped
/// into the target week); `field` is blame's name for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickConflict {
    pub run_id: String,
    pub field: String,
    pub expected: PickValue,
    pub current: PickValue,
}

/// What a pick would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickPlan {
    pub source_week: DateTime<Utc>,
    pub target_week: DateTime<Utc>,
    /// Empty when the target already holds every picked value.
    pub steps: Vec<PickStep>,
    pub conflicts: Vec<PickConflict>,
}

/// Why a record cannot be picked into that week.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickRefusal {
    /// Undo it with a revert instead.
    Rollback,
    /// Merges are reviewed multi-operation changes: pick their parts or
    /// draft again.
    Merge,
    /// The target week is the record's own week (use a revert or an edit).
    SameWeek,
    /// The target boss week has passed.
    WeekPassed,
    /// The weekly timing has no run in the target week.
    NoTargetRun {
        fixed_run_id: String,
    },
    /// The target run is done or cancelled (strict and force alike).
    TargetFinished {
        status: RunStatus,
    },
    UnsupportedPick {
        reason: String,
    },
}

impl fmt::Display for PickRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rollback => f.write_str("a rollback cannot be picked; revert instead"),
            Self::Merge => f.write_str("a merge cannot be picked"),
            Self::SameWeek => f.write_str("the change is already in that boss week"),
            Self::WeekPassed => f.write_str("that boss week has passed"),
            Self::NoTargetRun { fixed_run_id } => {
                write!(f, "weekly run {fixed_run_id} has no run in that boss week")
            }
            Self::TargetFinished { status } => {
                write!(f, "the target run is {}", status.as_str())
            }
            Self::UnsupportedPick { reason } => write!(f, "cannot pick {reason}"),
        }
    }
}

impl std::error::Error for PickRefusal {}

fn unsupported(reason: &str) -> PickRefusal {
    PickRefusal::UnsupportedPick {
        reason: reason.to_owned(),
    }
}

fn run_of(value: Option<&RowValue>) -> Option<&Run> {
    match value {
        Some(RowValue::Run(run)) => Some(run),
        _ => None,
    }
}

fn answer_of(value: Option<&RowValue>) -> Option<(RsvpState, RsvpSource)> {
    match value {
        Some(RowValue::Rsvp(row)) => Some((row.state, row.source)),
        _ => None,
    }
}

fn sorted(party: &[String]) -> Vec<String> {
    let set: BTreeSet<&String> = party.iter().collect();
    set.into_iter().cloned().collect()
}

fn calendar(_: impl fmt::Debug) -> PickRefusal {
    unsupported("a slot outside the calendar")
}

/// Weekly slots of one timing across weeks: `at`, a slot in the week
/// starting `from`, as the same local weekday and time in the week starting
/// `to`. A slot that is the timing's own placement in `from` (possibly
/// shifted by a DST gap) maps to the timing's placement in `to`, so a
/// shifted source never reads as a different time.
fn map_slot(
    at: DateTime<Utc>,
    from: &ZonedDateTime,
    to: &ZonedDateTime,
    timing: Option<&FixedRun>,
    policy: &SchedulePolicy,
) -> Result<DateTime<Utc>, PickRefusal> {
    let zone = policy.zone();
    let placed = |week: &ZonedDateTime, weekday: Weekday, time: NaiveTime| {
        slot_in_week(week, zone, weekday, time)
            .map(|slot| slot.to_fixed().with_timezone(&Utc))
            .map_err(calendar)
    };
    if let Some(timing) = timing
        && placed(from, timing.weekday, timing.time)? == at
    {
        return placed(to, timing.weekday, timing.time);
    }
    let local = at.astimezone(zone).map_err(calendar)?.wall();
    placed(to, local.weekday(), local.time())
}

/// One recorded answer change: member, before, after.
type AnswerRow<'a> = (&'a str, Option<&'a RowValue>, Option<&'a RowValue>);

/// Plan picking `record` into the boss week containing `target_week`, on
/// `current` (a whole-schedule snapshot) at `now`. Members to remove who
/// are not on the target (and members to add who already are) are dropped
/// from the party delta in every mode, so strict and forced picks (and the
/// preview) plan the same steps and conflicts; an emptied delta is no step.
///
/// # Errors
/// [`PickRefusal`].
pub fn plan_pick(
    record: &ChangeRecord,
    current: &ScheduleSnapshot,
    target_week: DateTime<Utc>,
    policy: &SchedulePolicy,
    now: DateTime<Utc>,
) -> Result<PickPlan, PickRefusal> {
    match record.origin.surface {
        Surface::Rollback => return Err(PickRefusal::Rollback),
        Surface::DraftMerge | Surface::RequestMerge => return Err(PickRefusal::Merge),
        // Tick changes (mark done, the attendance recount) are derived
        // statuses, not decisions to replay in another week.
        Surface::DeliveryTick => return Err(unsupported("a derived status")),
        _ => {}
    }
    let bad_week = |_| unsupported("a week outside the calendar");
    let target_zoned = policy.week_of(&target_week).map_err(bad_week)?;
    let target_instant = utc_instant(&target_zoned).map_err(bad_week)?;
    let this_week = utc_instant(&policy.week_of(&now).map_err(bad_week)?).map_err(bad_week)?;
    if target_instant < this_week {
        return Err(PickRefusal::WeekPassed);
    }
    // Source runs: the run rows the record changed, and the runs of the
    // answers it changed (looked up now: answer rows do not carry the week).
    let mut runs: BTreeMap<String, (&Run, &Run)> = BTreeMap::new();
    let mut answers: BTreeMap<String, Vec<AnswerRow<'_>>> = BTreeMap::new();
    for row in &record.rows {
        match &row.key {
            RowKey::FixedRun(_) => return Err(unsupported("a weekly timing change")),
            RowKey::Reminder(_) => {}
            RowKey::Run(id) => match (run_of(row.before.as_ref()), run_of(row.after.as_ref())) {
                (Some(before), Some(after)) => {
                    runs.insert(id.clone(), (before, after));
                }
                (None, _) => return Err(unsupported("a run's creation")),
                (_, None) => return Err(unsupported("a run's removal")),
            },
            RowKey::Rsvp { run_id, user_id } => answers.entry(run_id.clone()).or_default().push((
                user_id,
                row.before.as_ref(),
                row.after.as_ref(),
            )),
        }
    }
    let mut touched: BTreeSet<String> = runs.keys().cloned().collect();
    touched.extend(answers.keys().cloned());
    if touched.is_empty() {
        return Err(unsupported("a change with no run rows"));
    }
    // Every touched run, as it was: one source week, weekly timings only.
    let mut sources: BTreeMap<String, Run> = BTreeMap::new();
    for id in &touched {
        let source = match runs.get(id).copied() {
            Some((before, after)) => {
                if before.week_start != after.week_start {
                    return Err(unsupported("a move across boss weeks"));
                }
                if before.fixed_run_id != after.fixed_run_id {
                    return Err(unsupported("a change of weekly timing"));
                }
                before.clone()
            }
            None => current
                .runs
                .iter()
                .find(|run| &run.id == id)
                .cloned()
                .ok_or_else(|| unsupported("an answer on a run that no longer exists"))?,
        };
        if source.fixed_run_id.is_none() {
            return Err(unsupported("a one-off run (no weekly timing)"));
        }
        sources.insert(id.clone(), source);
    }
    let source_weeks: BTreeSet<DateTime<Utc>> =
        sources.values().map(|run| run.week_start).collect();
    if source_weeks.len() != 1 {
        return Err(unsupported("a change spanning several boss weeks"));
    }
    let source_week = source_weeks.into_iter().next().unwrap_or(target_instant);
    if source_week == target_instant {
        return Err(PickRefusal::SameWeek);
    }
    let mut steps = Vec::new();
    let mut conflicts = Vec::new();
    for (id, source) in &sources {
        let change = runs.get(id).copied();
        let fixed_run_id = source.fixed_run_id.clone().unwrap_or_default();
        let target = current
            .runs
            .iter()
            .find(|row| {
                row.fixed_run_id.as_deref() == Some(fixed_run_id.as_str())
                    && row.week_start == target_instant
            })
            .ok_or(PickRefusal::NoTargetRun {
                fixed_run_id: fixed_run_id.clone(),
            })?;
        if matches!(target.status, RunStatus::Done | RunStatus::Cancelled) {
            return Err(PickRefusal::TargetFinished {
                status: target.status,
            });
        }
        let timing = current.fixed_runs.iter().find(|row| row.id == fixed_run_id);
        let mut conflict = |field: &str, expected: PickValue, current: PickValue| {
            if expected != current {
                conflicts.push(PickConflict {
                    run_id: target.id.clone(),
                    field: field.to_owned(),
                    expected,
                    current,
                });
            }
        };
        let run_answers = answers.get(id).cloned().unwrap_or_default();
        let set_answers = run_answers
            .iter()
            .any(|(_, _, after)| answer_of(*after).is_some());
        let mut party = target.participants.clone();
        let mut removed: Vec<String> = Vec::new();
        if let Some((before, after)) = change {
            if before.bosses != after.bosses {
                return Err(unsupported("a bosses change"));
            }
            if before.channel_id != after.channel_id {
                return Err(unsupported("a channel change"));
            }
            let moved = before.datetime != after.datetime;
            let party_changed = sorted(&before.participants) != sorted(&after.participants);
            if moved {
                if target.status == RunStatus::Otot {
                    return Err(unsupported("moving an otot run"));
                }
                let from = policy.week_of(&before.week_start).map_err(bad_week)?;
                let expected = map_slot(before.datetime, &from, &target_zoned, timing, policy)?;
                let to = map_slot(after.datetime, &from, &target_zoned, timing, policy)?;
                if target.datetime != to {
                    conflict(
                        "slot",
                        PickValue::Slot(expected),
                        PickValue::Slot(target.datetime),
                    );
                    steps.push(PickStep::Amend {
                        run_id: target.id.clone(),
                        to,
                    });
                }
            }
            if party_changed {
                removed = before
                    .participants
                    .iter()
                    .filter(|user| !after.participants.contains(user))
                    .cloned()
                    .collect();
                let mut remove = removed.clone();
                let mut add: Vec<String> = after
                    .participants
                    .iter()
                    .filter(|user| !before.participants.contains(user))
                    .cloned()
                    .collect();
                remove.retain(|user| target.participants.contains(user));
                add.retain(|user| !target.participants.contains(user));
                party.retain(|user| !remove.contains(user));
                party.extend(
                    add.iter()
                        .filter(|user| !party.contains(user))
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                let target_set = sorted(&target.participants);
                if target_set != sorted(&after.participants)
                    && !(remove.is_empty() && add.is_empty())
                {
                    conflict(
                        "participants",
                        PickValue::Party(sorted(&before.participants)),
                        PickValue::Party(target_set),
                    );
                    steps.push(PickStep::Swap {
                        run_id: target.id.clone(),
                        remove,
                        add,
                    });
                }
            }
            // A status is picked only as the run's sole change; alongside a
            // move, a swap or answers it is derived from them.
            if before.status != after.status && !moved && !party_changed && !set_answers {
                if after.status == RunStatus::AtRisk {
                    return Err(unsupported("a derived status"));
                }
                if target.status != after.status {
                    conflict(
                        "status",
                        PickValue::Status(before.status),
                        PickValue::Status(target.status),
                    );
                    steps.push(PickStep::Status {
                        run_id: target.id.clone(),
                        status: after.status,
                    });
                }
            }
        }
        // Clearing answers of members the record took off the run, or of a
        // run coming back to planned, are side effects the swap or status
        // step repeats on the target.
        let revived = change.is_some_and(|(before, after)| {
            matches!(
                before.status,
                RunStatus::Cancelled | RunStatus::Otot | RunStatus::Done
            ) && after.status == RunStatus::Planned
        });
        for (user_id, before, after) in run_answers {
            let Some((state, source)) = answer_of(after) else {
                let side_effect = removed.iter().any(|user| user == user_id)
                    || (revived
                        && change.is_some_and(|(before, _)| {
                            before.participants.iter().any(|user| user == user_id)
                        }));
                if side_effect {
                    continue;
                }
                return Err(unsupported("a cleared answer"));
            };
            if !party.iter().any(|user| user == user_id) {
                return Err(unsupported("an answer by a member not on the target run"));
            }
            let now_state = current
                .rsvps
                .iter()
                .find(|row| row.run_id == target.id && row.user_id == user_id)
                .map(|row| row.state);
            if now_state == Some(state) {
                continue;
            }
            conflict(
                &format!("rsvp:{user_id}"),
                PickValue::Answer(answer_of(before).map(|(state, _)| state)),
                PickValue::Answer(now_state),
            );
            steps.push(PickStep::Rsvp {
                run_id: target.id.clone(),
                user_id: user_id.to_owned(),
                state,
                source,
            });
        }
    }
    Ok(PickPlan {
        source_week,
        target_week: target_instant,
        steps,
        conflicts,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::domain::history::{Actor, ChangeMeta, Origin, RowChange};
    use crate::domain::schedule::{ReminderPolicy, Rsvp, RunSource};

    fn utc(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, day, hour, 0, 0).unwrap()
    }

    /// Weeks reset Thursday 00:00 UTC: week 0 from 3 Sep, week 1 from 10 Sep.
    fn policy() -> SchedulePolicy {
        SchedulePolicy::new(
            ReminderPolicy {
                zone: chrono_tz::UTC,
                ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            Weekday::Thu,
            NaiveTime::MIN,
        )
    }

    fn run(id: &str, week: u32, party: &[&str], status: RunStatus) -> Run {
        Run {
            id: id.into(),
            fixed_run_id: Some("f".into()),
            channel_id: Some("900".into()),
            week_start: utc(3 + 7 * week, 0),
            datetime: utc(5 + 7 * week, 20),
            bosses: vec!["HFA".into()],
            participants: party.iter().map(|user| (*user).to_owned()).collect(),
            status,
            source: RunSource::Fixed,
            attendance: Vec::new(),
            status_pin: None,
        }
    }

    fn answer(run: &str, user: &str, state: RsvpState) -> Rsvp {
        Rsvp {
            run_id: run.into(),
            user_id: user.into(),
            state,
            source: RsvpSource::Reaction,
            at: utc(4, 0),
        }
    }

    fn record(rows: Vec<RowChange>) -> ChangeRecord {
        let meta = ChangeMeta {
            origin: Origin::new(Actor::admin("a"), Surface::AdminPortal),
            at: utc(4, 1),
            notices: Vec::new(),
            refs: Vec::new(),
            request_digest: None,
            expect: Expect::default(),
        };
        ChangeRecord::seal(
            Some((0, &"0".repeat(64))),
            "r".into(),
            2,
            meta,
            Vec::new(),
            rows,
        )
        .unwrap()
    }

    fn run_row(before: Run, after: Run) -> RowChange {
        RowChange {
            key: RowKey::Run(before.id.clone()),
            before: Some(RowValue::Run(before)),
            after: Some(RowValue::Run(after)),
        }
    }

    fn answer_row(
        run: &str,
        user: &str,
        before: Option<RsvpState>,
        after: Option<RsvpState>,
    ) -> RowChange {
        RowChange {
            key: RowKey::Rsvp {
                run_id: run.into(),
                user_id: user.into(),
            },
            before: before.map(|state| RowValue::Rsvp(answer(run, user, state))),
            after: after.map(|state| RowValue::Rsvp(answer(run, user, state))),
        }
    }

    fn current(runs: Vec<Run>, answers: Vec<Rsvp>) -> ScheduleSnapshot {
        ScheduleSnapshot {
            runs,
            rsvps: answers,
            ..ScheduleSnapshot::default()
        }
    }

    fn plan(
        record: &ChangeRecord,
        now: &ScheduleSnapshot,
        _force: bool,
    ) -> Result<PickPlan, PickRefusal> {
        // Planning is the same in both modes.
        plan_pick(record, now, utc(10, 0), &policy(), utc(4, 0))
    }

    #[test]
    fn a_swap_skips_the_leavers_cleared_answer() {
        let before = run("s", 0, &["1", "2"], RunStatus::Planned);
        let mut after = before.clone();
        after.participants = vec!["1".into(), "3".into()];
        let picked = record(vec![
            run_row(before, after),
            answer_row("s", "2", Some(RsvpState::Yes), None),
        ]);
        let target = run("t", 1, &["1", "2"], RunStatus::Planned);
        let planned = plan(&picked, &current(vec![target], Vec::new()), false).unwrap();
        assert_eq!(
            planned.steps,
            [PickStep::Swap {
                run_id: "t".into(),
                remove: vec!["2".into()],
                add: vec!["3".into()]
            }]
        );
        assert!(planned.conflicts.is_empty());
    }

    #[test]
    fn a_sole_status_is_picked_and_a_revival_takes_its_cleared_answers() {
        let before = run("s", 0, &["1", "2"], RunStatus::Planned);
        let mut after = before.clone();
        after.status = RunStatus::Otot;
        let picked = record(vec![run_row(before.clone(), after.clone())]);
        let target = run("t", 1, &["1", "2"], RunStatus::Planned);
        let planned = plan(&picked, &current(vec![target], Vec::new()), false).unwrap();
        assert_eq!(
            planned.steps,
            [PickStep::Status {
                run_id: "t".into(),
                status: RunStatus::Otot
            }]
        );
        // Otot back to planned clears answers: they follow the status.
        let revived = record(vec![
            run_row(after, before),
            answer_row("s", "1", Some(RsvpState::Yes), None),
        ]);
        let target = run("t", 1, &["1", "2"], RunStatus::Otot);
        let planned = plan(&revived, &current(vec![target], Vec::new()), false).unwrap();
        assert_eq!(
            planned.steps,
            [PickStep::Status {
                run_id: "t".into(),
                status: RunStatus::Planned
            }]
        );
        // Any other cleared answer is refused.
        let cleared = record(vec![answer_row("s", "1", Some(RsvpState::Yes), None)]);
        let source = run("s", 0, &["1", "2"], RunStatus::Planned);
        let target = run("t", 1, &["1", "2"], RunStatus::Planned);
        assert!(matches!(
            plan(&cleared, &current(vec![source, target], Vec::new()), false),
            Err(PickRefusal::UnsupportedPick { .. })
        ));
    }

    #[test]
    fn finished_and_otot_targets() {
        let before = run("s", 0, &["1", "2"], RunStatus::Planned);
        let mut after = before.clone();
        after.datetime = utc(5, 21);
        let moved = record(vec![run_row(before, after)]);
        for status in [RunStatus::Done, RunStatus::Cancelled] {
            let target = run("t", 1, &["1", "2"], status);
            for force in [false, true] {
                assert_eq!(
                    plan(&moved, &current(vec![target.clone()], Vec::new()), force),
                    Err(PickRefusal::TargetFinished { status })
                );
            }
        }
        let otot = run("t", 1, &["1", "2"], RunStatus::Otot);
        assert!(matches!(
            plan(&moved, &current(vec![otot.clone()], Vec::new()), true),
            Err(PickRefusal::UnsupportedPick { .. })
        ));
        // An answer onto an otot run is fine.
        let source = run("s", 0, &["1", "2"], RunStatus::Planned);
        let answered = record(vec![answer_row("s", "2", None, Some(RsvpState::No))]);
        let planned = plan(&answered, &current(vec![source, otot], Vec::new()), false).unwrap();
        assert_eq!(planned.steps.len(), 1);
    }

    #[test]
    fn values_already_there_are_skipped_and_force_trims_the_delta() {
        let before = run("s", 0, &["1", "2"], RunStatus::Planned);
        let mut after = before.clone();
        after.datetime = utc(5, 21);
        after.participants = vec!["1".into(), "3".into()];
        let picked = record(vec![run_row(before, after.clone())]);
        // The target already looks like the pick's result: nothing to do.
        let mut done = run("t", 1, &["1", "3"], RunStatus::Planned);
        done.datetime = utc(12, 21);
        let planned = plan(&picked, &current(vec![done], Vec::new()), false).unwrap();
        assert!(planned.steps.is_empty() && planned.conflicts.is_empty());
        // 2 already left the target: force drops them from `remove`.
        let target = run("t", 1, &["1", "4"], RunStatus::Planned);
        let planned = plan(&picked, &current(vec![target], Vec::new()), true).unwrap();
        assert!(planned.steps.contains(&PickStep::Swap {
            run_id: "t".into(),
            remove: Vec::new(),
            add: vec!["3".into()]
        }));
        assert!(
            planned
                .conflicts
                .iter()
                .any(|conflict| conflict.field == "participants")
        );
    }

    #[test]
    fn a_shifted_source_slot_maps_to_the_timings_target_slot() {
        let zone = chrono_tz::Europe::London;
        let policy = SchedulePolicy::new(
            ReminderPolicy {
                zone,
                ping_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            Weekday::Thu,
            NaiveTime::MIN,
        );
        let timing = FixedRun {
            id: "f".into(),
            owner_id: "1".into(),
            channel_id: None,
            bosses: vec!["HFA".into()],
            weekday: Weekday::Sun,
            time: NaiveTime::from_hms_opt(1, 30, 0).unwrap(),
            participants: vec!["1".into()],
            note: None,
            attendance_default: Default::default(),
            standing: Vec::new(),
        };
        let week = |text: &str| {
            policy
                .week_of(
                    &DateTime::parse_from_rfc3339(text)
                        .unwrap()
                        .with_timezone(&Utc),
                )
                .unwrap()
        };
        // 01:30 on 29 Mar 2026 does not exist in London; the source week
        // placed it elsewhere, the target week at 01:30 BST.
        let source_week = week("2026-03-27T12:00:00Z");
        let target_week = week("2026-04-03T12:00:00Z");
        let shifted = slot_in_week(&source_week, zone, Weekday::Sun, timing.time)
            .unwrap()
            .to_fixed()
            .with_timezone(&Utc);
        let target = map_slot(shifted, &source_week, &target_week, Some(&timing), &policy).unwrap();
        assert_eq!(
            target,
            DateTime::parse_from_rfc3339("2026-04-05T00:30:00Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }
}
