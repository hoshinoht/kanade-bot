use std::collections::HashMap;

use chrono::TimeZone;

use super::*;
use crate::domain::{
    characters::{Character, CharacterRole},
    history::Actor,
};

/// Tue 29 Sep 2026 04:00 UTC; the boss week started Thu 24 Sep.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 29, 4, 0, 0).unwrap()
}

fn current_week() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 24, 0, 0, 0).unwrap()
}

fn next_week() -> DateTime<Utc> {
    current_week() + TimeDelta::days(7)
}

/// Invented roster character `id` named `name`.
fn subject(id: &str, name: &str) -> Subject {
    let character = Character {
        id: id.into(),
        member_id: format!("m-{id}"),
        ign: name.into(),
        world: "msea".into(),
        class: None,
        level: None,
        role: CharacterRole::Main,
        retired_at: None,
        created_at: now(),
        updated_at: now(),
        updated_by: Actor::system("test"),
        version: 1,
    };
    Subject {
        character_id: character.id.clone(),
        member_id: character.member_id.clone(),
        ign: RosterIgn::of(&character).unwrap(),
    }
}

fn run(id: &str, in_hours: i64, subjects: Vec<Subject>) -> UpcomingRun {
    let datetime = now() + TimeDelta::hours(in_hours);
    UpcomingRun {
        run_id: id.into(),
        bosses: vec!["HLimbo".into()],
        status: RunStatus::Planned,
        datetime,
        week_start: if datetime < next_week() {
            current_week()
        } else {
            next_week()
        },
        subjects,
        missing: Vec::new(),
        parties: Vec::new(),
    }
}

fn upcoming(runs: Vec<UpcomingRun>) -> Upcoming {
    Upcoming {
        current_week: current_week(),
        runs,
    }
}

/// Results held in memory, by IGN key and IGN set.
#[derive(Default)]
struct Held {
    members: HashMap<String, (DateTime<Utc>, Vec<String>)>,
    parties: HashMap<String, (DateTime<Utc>, Vec<String>)>,
}

impl Held {
    /// `subjects` and their total, each fetched `age` ago.
    fn with(mut self, subjects: &[Subject], age: TimeDelta) -> Self {
        let at = now() - age;
        for subject in subjects {
            self.members
                .insert(subject.ign.key(), (at, vec![subject.character_id.clone()]));
        }
        self.parties.insert(
            set_key(subjects.iter().map(|subject| &subject.ign)),
            (at, characters(subjects)),
        );
        self
    }
}

impl Memory for Held {
    fn member(&self, ign: &RosterIgn) -> Option<Reading<'_>> {
        self.members.get(&ign.key()).map(|(at, ids)| Reading {
            fetched_at: *at,
            characters: ids,
        })
    }

    fn party(&self, set_key: &str) -> Option<Reading<'_>> {
        self.parties.get(set_key).map(|(at, ids)| Reading {
            fetched_at: *at,
            characters: ids,
        })
    }
}

fn fresh(at: DateTime<Utc>) -> bool {
    now() - at < TimeDelta::days(7)
}

fn pick(runs: Vec<UpcomingRun>, held: &Held, cap_left: usize) -> Selection {
    select(upcoming(runs), now(), fresh, held, |_| false, cap_left)
}

fn queued(selection: &Selection) -> Vec<&str> {
    selection
        .queued
        .iter()
        .map(|due| due.run.run_id.as_str())
        .collect()
}

fn ren() -> Subject {
    subject("ch-1", "RenShade")
}

fn mika() -> Subject {
    subject("ch-2", "MikaBish")
}

#[test]
fn only_runs_starting_within_seven_days_are_in_the_horizon() {
    let runs = vec![
        run("past", -1, vec![ren()]),
        run("now", 0, vec![ren()]),
        run("day7", 7 * 24, vec![mika()]),
        run("day8", 8 * 24, vec![mika()]),
    ];
    let selection = pick(runs, &Held::default(), 60);
    assert_eq!(selection.runs, 2);
    assert_eq!(queued(&selection), ["now", "day7"]);
}

#[test]
fn only_live_runs_are_refreshed() {
    let mut runs = Vec::new();
    for (at, status) in [
        RunStatus::Planned,
        RunStatus::Confirmed,
        RunStatus::AtRisk,
        RunStatus::Done,
        RunStatus::Cancelled,
    ]
    .into_iter()
    .enumerate()
    {
        let mut row = run(status.as_str(), i64::try_from(at).unwrap() + 1, vec![ren()]);
        row.status = status;
        runs.push(row);
    }
    let selection = pick(runs, &Held::default(), 60);
    assert_eq!(queued(&selection), ["planned", "confirmed", "at_risk"]);
}

/// Own time is played any time that week: the current week's own-time
/// runs are refreshed whatever their slot, the next week's are not.
#[test]
fn own_time_runs_of_the_current_week_are_included() {
    let mut earlier = run("otot-now", -48, vec![ren()]);
    earlier.status = RunStatus::Otot;
    let mut next = run("otot-next", 4 * 24, vec![mika()]);
    next.status = RunStatus::Otot;
    assert_eq!(next.week_start, next_week());
    let selection = pick(vec![earlier, next], &Held::default(), 60);
    assert_eq!(queued(&selection), ["otot-now"]);
}

#[test]
fn a_run_is_due_while_a_member_or_its_total_is_missing_or_stale() {
    let party = vec![ren(), mika()];
    let due = |held: &Held| pick(vec![run("r1", 2, party.clone())], held, 60).due;
    let kept = || Held::default().with(&party, TimeDelta::days(6));
    assert_eq!(due(&kept()), 0, "everything fresh");
    assert_eq!(due(&Held::default()), 1, "nothing held");
    assert_eq!(
        due(&Held::default().with(&party, TimeDelta::days(7))),
        1,
        "7 days old"
    );
    let mut one_missing = kept();
    one_missing.members.remove(&mika().ign.key());
    assert_eq!(due(&one_missing), 1, "a member missing");
    let mut other_character = kept();
    other_character
        .members
        .insert(ren().ign.key(), (now(), vec!["ch-9".into()]));
    assert_eq!(
        due(&other_character),
        1,
        "the IGN's result is another character's"
    );
    let mut no_total = kept();
    no_total.parties.clear();
    assert_eq!(due(&no_total), 1, "the total missing");
    let mut stale_total = kept();
    for held in stale_total.parties.values_mut() {
        held.0 = now() - TimeDelta::days(8);
    }
    assert_eq!(due(&stale_total), 1, "the total stale");
    // The total of another IGN set (Ren alone) does not count.
    let solo = Held::default().with(&[ren()], TimeDelta::days(1));
    let mut members_only = kept();
    members_only.parties = solo.parties;
    assert_eq!(due(&members_only), 1, "only another set's total");
}

/// Only the stale members are counted as lookups: the rest are reused.
#[test]
fn lookups_count_only_the_members_to_ask_for() {
    let held = Held::default().with(&[ren()], TimeDelta::days(1));
    let selection = pick(vec![run("r1", 2, vec![ren(), mika()])], &held, 60);
    assert_eq!(selection.queued[0].lookups, 1);
}

#[test]
fn due_runs_are_queued_soonest_first() {
    let runs = vec![
        run("late", 30, vec![ren()]),
        run("soon", 3, vec![mika()]),
        run("mid", 10, vec![subject("ch-3", "AoiLyn")]),
    ];
    let selection = pick(runs, &Held::default(), 60);
    assert_eq!(queued(&selection), ["soon", "mid", "late"]);
}

#[test]
fn runs_with_nobody_or_too_many_to_look_up_are_skipped() {
    let seven: Vec<Subject> = (0..=MAX_BATCH)
        .map(|at| subject(&format!("ch-{at}"), &format!("Member{at}")))
        .collect();
    let runs = vec![run("empty", 1, Vec::new()), run("seven", 2, seven)];
    let selection = pick(runs, &Held::default(), 60);
    assert_eq!((selection.runs, selection.due), (2, 0));
    assert!(selection.queued.is_empty());
}

/// Queued while the lookups fit the cap; a member two runs share counts
/// once; past the first run that does not fit, nothing more is queued.
#[test]
fn the_cap_limits_what_is_queued() {
    let runs = vec![
        run("a", 1, vec![ren(), mika()]),
        run("b", 2, vec![ren()]),
        run(
            "c",
            3,
            vec![subject("ch-3", "AoiLyn"), subject("ch-4", "KaiLyn")],
        ),
        run("d", 4, vec![subject("ch-5", "SoraPF")]),
    ];
    let selection = pick(runs, &Held::default(), 3);
    assert_eq!(queued(&selection), ["a", "b"]);
    assert_eq!(
        selection
            .queued
            .iter()
            .map(|due| due.lookups)
            .collect::<Vec<_>>(),
        [2, 0]
    );
    assert_eq!(selection.due, 4, "the capped runs are still due");
    // Nothing left: only a run that needs no `ign=` lookup would fit.
    let mut no_total = Held::default().with(&[ren()], TimeDelta::days(1));
    no_total.parties.clear();
    let selection = pick(vec![run("b", 2, vec![ren()])], &no_total, 0);
    assert_eq!(queued(&selection), ["b"]);
}

#[test]
fn a_backed_off_run_is_not_queued() {
    let runs = vec![run("failed", 1, vec![ren()]), run("ok", 2, vec![mika()])];
    let selection = select(
        upcoming(runs),
        now(),
        fresh,
        &Held::default(),
        |run| run == "failed",
        60,
    );
    assert_eq!(queued(&selection), ["ok"]);
    assert_eq!(selection.due, 2);
}
