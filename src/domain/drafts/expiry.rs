//! A draft's expiry week, derived from its staged operations: the earliest
//! boss week any run-level operation touches. Weekly-timing-only drafts
//! never expire by week.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::op::{DraftOp, resolve};
use crate::domain::schedule::{Draft, SchedulePolicy, ScheduleSnapshot, utc_instant};

/// The earliest boss week (`policy.week_of`) touched by any run-level
/// operation in `ops` (`created` resolves their targets, as at replay):
/// a created run's slot (and its stated `week_start`); an amended run's source week and destination slot
/// week; any other run operation's run week. Operations on weekly timings
/// only (`AddFixedRun`, `ApplyFixedEdit`, `RetireFixedRun`) contribute no
/// week, so a draft staging only those never expires by week.
///
/// Run weeks come from the replayed `draft` (which holds created rows),
/// falling back to the `base` snapshot; an amended run's source week comes
/// from the base first. Unresolvable targets are skipped: staging only
/// accepts resolvable operations.
pub fn expires_week(
    ops: &[DraftOp],
    created: &[Option<String>],
    base: &ScheduleSnapshot,
    draft: &Draft,
    policy: &SchedulePolicy,
) -> Option<DateTime<Utc>> {
    let base_weeks: BTreeMap<&str, DateTime<Utc>> = base
        .runs
        .iter()
        .map(|run| (run.id.as_str(), run.week_start))
        .collect();
    let mut weeks: Vec<DateTime<Utc>> = Vec::new();
    let slot_week = |at: &DateTime<Utc>| {
        policy
            .week_of(at)
            .ok()
            .and_then(|start| utc_instant(&start).ok())
    };
    let run_week = |id: &str| {
        draft
            .run(id)
            .or_else(|| base.runs.iter().find(|run| run.id == id))
    };
    for op in ops {
        match op {
            DraftOp::AddFixedRun(_)
            | DraftOp::ApplyFixedEdit { .. }
            | DraftOp::FixedParticipants { .. }
            | DraftOp::RetireFixedRun { .. } => {}
            DraftOp::CreateRun {
                datetime,
                week_start,
                ..
            } => {
                // Staging refuses a week_start that is not the slot's week;
                // take the earlier anyway so a stale row cannot outlive it.
                weeks.extend(slot_week(datetime));
                weeks.push(*week_start);
            }
            DraftOp::AmendRun { run, to } => {
                // The source week is the run's week before the amendment.
                let source = resolve(run, created).ok().and_then(|id| {
                    base_weeks
                        .get(id.as_str())
                        .copied()
                        .or_else(|| draft.run(&id).map(|run| run.week_start))
                });
                weeks.extend(source);
                weeks.extend(slot_week(to));
            }
            DraftOp::SetStatus { run, .. }
            | DraftOp::SwapParticipants { run, .. }
            | DraftOp::SetRsvp { run, .. }
            | DraftOp::ResetToFixed { run } => {
                let week = resolve(run, created)
                    .ok()
                    .and_then(|id| run_week(&id).map(|run| run.week_start));
                weeks.extend(week);
            }
        }
    }
    weeks.into_iter().min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::drafts::Target;
    use crate::domain::schedule::{RunSource, RunStatus};

    fn week(at: &DateTime<Utc>, policy: &SchedulePolicy) -> DateTime<Utc> {
        utc_instant(&policy.week_of(at).unwrap()).unwrap()
    }

    #[test]
    fn run_ops_contribute_their_weeks_and_fixed_ops_contribute_none() {
        let policy = SchedulePolicy::new(
            crate::domain::schedule::ReminderPolicy {
                zone: chrono_tz::UTC,
                ping_time: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            chrono::Weekday::Wed,
            chrono::NaiveTime::from_hms_opt(16, 0, 0).unwrap(),
        );
        let w1 = DateTime::parse_from_rfc3339("2026-08-26T16:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let w2 = DateTime::parse_from_rfc3339("2026-09-02T16:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let slot = DateTime::parse_from_rfc3339("2026-08-31T13:30:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let moved = DateTime::parse_from_rfc3339("2026-09-03T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(week(&slot, &policy), w1);
        assert_eq!(week(&moved, &policy), w2);
        let mut base = ScheduleSnapshot::default();
        base.runs.push(crate::domain::schedule::Run {
            id: "r-1".into(),
            fixed_run_id: None,
            channel_id: None,
            week_start: w1,
            datetime: slot,
            bosses: vec!["HFA".into()],
            participants: vec!["1".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
            attendance: Vec::new(),
            status_pin: None,
        });
        let draft = Draft::new(base.clone());
        let rate = |run: Target| DraftOp::SetRsvp {
            run,
            user_id: "1".into(),
            state: crate::domain::schedule::RsvpState::Yes,
            source: crate::domain::schedule::RsvpSource::Chat,
        };
        // A run operation expires at its run's week.
        assert_eq!(
            expires_week(
                &[rate(Target::Existing("r-1".into()))],
                &[],
                &base,
                &draft,
                &policy
            ),
            Some(w1)
        );
        // An amendment spans its source week and destination week.
        assert_eq!(
            expires_week(
                &[DraftOp::AmendRun {
                    run: Target::Existing("r-1".into()),
                    to: moved,
                }],
                &[],
                &base,
                &draft,
                &policy
            ),
            Some(w1)
        );
        // The earliest week wins.
        assert_eq!(
            expires_week(
                &[
                    rate(Target::Existing("r-1".into())),
                    DraftOp::CreateRun {
                        fixed: None,
                        channel_id: None,
                        week_start: w2,
                        datetime: moved,
                        bosses: vec!["HFA".into()],
                        participants: vec!["1".into()],
                        status: RunStatus::Planned,
                        source: RunSource::Amend,
                    },
                ],
                &[],
                &base,
                &draft,
                &policy
            ),
            Some(w1)
        );
        // Fixed-only operations never expire by week.
        assert_eq!(
            expires_week(
                &[DraftOp::RetireFixedRun {
                    fixed: Target::Existing("f-1".into()),
                    weeks: vec![w1],
                }],
                &[],
                &base,
                &draft,
                &policy
            ),
            None
        );
        // Amending into an earlier week expires at the destination.
        let mut later = base.clone();
        later.runs[0].week_start = w2;
        later.runs[0].datetime = moved;
        assert_eq!(
            expires_week(
                &[DraftOp::AmendRun {
                    run: Target::Existing("r-1".into()),
                    to: slot,
                }],
                &[],
                &later,
                &Draft::new(later.clone()),
                &policy
            ),
            Some(w1)
        );
        // A created run's amendment: its creation week and destination.
        let create = |at: DateTime<Utc>, week: DateTime<Utc>| DraftOp::CreateRun {
            fixed: None,
            channel_id: None,
            week_start: week,
            datetime: at,
            bosses: vec!["HFA".into()],
            participants: vec!["1".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        };
        let mut created = base.clone();
        created.runs[0].id = "c-1".into();
        created.runs[0].week_start = w1;
        created.runs[0].datetime = slot;
        let amend_created = |to: DateTime<Utc>| DraftOp::AmendRun {
            run: Target::Created(0),
            to,
        };
        for (ops, want) in [
            (vec![create(moved, w2), amend_created(slot)], w1),
            (vec![create(slot, w1), amend_created(moved)], w1),
        ] {
            assert_eq!(
                expires_week(
                    &ops,
                    &[Some("c-1".into()), None],
                    &ScheduleSnapshot::default(),
                    &Draft::new(created.clone()),
                    &policy
                ),
                Some(want)
            );
        }
        // A stale stated week_start earlier than the slot's still counts.
        assert_eq!(
            expires_week(&[create(moved, w1)], &[None], &base, &draft, &policy),
            Some(w1)
        );
        // Unresolvable targets are skipped, not fatal.
        assert_eq!(
            expires_week(
                &[rate(Target::Created(3))],
                &[None, None],
                &base,
                &draft,
                &policy
            ),
            None
        );
    }

    #[test]
    fn slot_weeks_follow_the_zone_across_a_dst_change() {
        // London springs forward on Sun 29 Mar 2026: the Wed 00:00 reset
        // after it is 23:00 UTC on Tue 31 Mar, not 00:00 UTC on 1 Apr.
        let policy = SchedulePolicy::new(
            crate::domain::schedule::ReminderPolicy {
                zone: chrono_tz::Europe::London,
                ping_time: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
                countdowns: vec![60],
            },
            chrono::Weekday::Wed,
            chrono::NaiveTime::MIN,
        );
        let at = |text: &str| {
            DateTime::parse_from_rfc3339(text)
                .unwrap()
                .with_timezone(&Utc)
        };
        let create = |slot: DateTime<Utc>, week: DateTime<Utc>| DraftOp::CreateRun {
            fixed: None,
            channel_id: None,
            week_start: week,
            datetime: slot,
            bosses: vec!["HFA".into()],
            participants: vec!["1".into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        };
        let base = ScheduleSnapshot::default();
        let draft = Draft::new(base.clone());
        let before = at("2026-03-25T00:00:00+00:00");
        let after = at("2026-03-31T23:00:00+00:00");
        for (slot, week) in [
            (at("2026-03-31T22:30:00+00:00"), before),
            (at("2026-03-31T23:30:00+00:00"), after),
        ] {
            // The stated week is the later one, so only the slot's own week
            // can produce the earlier answer.
            assert_eq!(
                expires_week(&[create(slot, after)], &[None], &base, &draft, &policy),
                Some(week)
            );
        }
    }
}
