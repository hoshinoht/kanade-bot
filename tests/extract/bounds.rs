//! D-EXTRACT-BOUND: "after"/"before" a day or time bounds a `move`'s run
//! instead of naming its new slot; day-of-month dates and leading-zero
//! 24-hour clocks resolve as written. Invented runs and ids; the message is
//! the 2026-10-15 report's own wording.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, NaiveDate, NaiveTime, Utc, Weekday};
use kanade_domain::history::Origin;
use kanade_domain::schedule::{NewRun, Run, RunSource, RunStatus};
use kanade_domain::scheduler::{ScheduleStore, Scope};
use kanade_extract::backlog::BacklogEntry;
use kanade_extract::plan::{BurstInputs, Plan, plan_burst};
use kanade_extract::schema::{Extraction, parse_response};
use kanade_extract::{Amendment, AmendmentKind};

use crate::fakes::{ALVIN, CHANNEL, MY, World, local, message, reply, zone};

const MOVED: &str = "I moved all the runs to after 0930 17th for now";
const WITHIN: &str = "already within the stated bound";

/// Thu 15 Oct 2026 18:57, when the message was sent.
fn sent() -> DateTime<Utc> {
    local(10, 15, 18, 57)
}

/// The boss week (Thursday reset) holding `day` of October.
fn week_of(day: u32) -> DateTime<Utc> {
    local(10, if day < 22 { 15 } else { 22 }, 0, 0)
}

/// Every run that week, already after Sat 17 09:30: (day, hour, minute, boss).
const WEEK: [(u32, u32, u32, &str); 10] = [
    (17, 22, 0, "HCarling"),
    (17, 23, 0, "XKalos"),
    (17, 23, 30, "HLimbo"),
    (18, 21, 0, "HMaleficStar"),
    (19, 23, 30, "HFA"),
    (22, 22, 0, "HCarling"),
    (22, 22, 30, "XKalos"),
    (22, 23, 30, "HLimbo"),
    (24, 22, 0, "HMaleficStar"),
    (24, 23, 0, "HFA"),
];

/// The model's reply: one `move` per run, each citing it.
fn moves(runs: &[(String, &str)], day_ref: &str, time_ref: Option<&str>) -> String {
    let time_ref = time_ref.map_or("null".to_owned(), |time| format!("{time:?}"));
    let amendments: Vec<String> = runs
        .iter()
        .map(|(id, boss)| {
            format!(
                r##"{{"kind":"move","bosses":["{boss}"],"day_ref":{day_ref:?},"time_ref":{time_ref},"target_run_hint":"#{id}","participants":[],"confidence":0.9,"evidence_message_ids":["101"]}}"##
            )
        })
        .collect();
    format!(
        r#"{{"amendments":[{}],"summary":"runs moved after Sat 09:30"}}"#,
        amendments.join(",")
    )
}

fn plan_at<'a>(extraction: &Extraction, runs: &[&'a Run], anchor: DateTime<Utc>) -> Plan<'a> {
    let order = vec!["101".to_owned()];
    let times = HashMap::from([("101".to_owned(), anchor)]);
    let authors = HashMap::from([("101".to_owned(), MY.to_owned())]);
    plan_burst(
        extraction,
        &BurstInputs {
            anchor,
            now: anchor,
            zone: zone(),
            reset_weekday: Weekday::Thu,
            reset_time: NaiveTime::MIN,
            channel_runs: runs,
            guild_runs: runs,
            burst_order: &order,
            author_ids: &authors,
            message_times: &times,
            min_confidence: 0.6,
            boss_table: None,
            run_ends: None,
        },
    )
    .expect("in range")
}

#[tokio::test(start_paused = true)]
async fn moving_every_run_after_a_time_they_already_follow_proposes_nothing() {
    // The fixture's own two runs come first, so this week's ids continue from 3.
    let ids: Vec<String> = (3..=12)
        .map(|n| format!("0000d4af-0000-4000-8000-{n:012}"))
        .collect();
    let cited: Vec<(String, &str)> = ids
        .iter()
        .cloned()
        .zip(WEEK.iter().map(|run| run.3))
        .collect();
    let world = World::new(vec![reply(&moves(&cited, "17th", Some("after 0930")))]).await;
    world.scheduler.clock.set(sent().fixed_offset());
    for ((day, hour, minute, boss), id) in WEEK.iter().zip(&ids) {
        let created = world
            .scheduler
            .service()
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: None,
                channel_id: Some(CHANNEL.into()),
                week_start: week_of(*day),
                datetime: local(10, *day, *hour, *minute),
                bosses: vec![(*boss).into()],
                participants: vec![MY.into(), ALVIN.into()],
                status: RunStatus::Planned,
                source: RunSource::Amend,
            })
            .await
            .expect("create_run");
        assert_eq!(&created, id);
    }
    let said = message("101", MY, sent(), MOVED);
    world.extractor.store_message(&said).await.expect("cache");
    let report = world
        .extractor
        .flush(
            CHANNEL,
            &[BacklogEntry {
                channel_id: CHANNEL.into(),
                message_id: said.id.clone(),
                created_at: said.created_at,
            }],
        )
        .await;

    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(world.requests(), 1);
    assert!(report.proposals.is_empty(), "{report:?}");
    assert!(report.refused.is_empty(), "{report:?}");
    assert!(world.live_proposals().await.is_empty());
    assert!(world.outbox.cards.lock().unwrap().is_empty());

    // Replan the recorded reply over the same schedule to see why each went.
    let extraction = parse_response(&world.logs().await[0].raw_response).expect("recorded reply");
    let snapshot = world.store.load(&Scope::All).await.expect("schedule");
    let runs: Vec<&Run> = snapshot
        .runs
        .iter()
        .filter(|run| ids.contains(&run.id))
        .collect();
    let plan = plan_at(&extraction, &runs, sent());
    assert!(plan.planned.is_empty());
    assert!(
        plan.dropped
            .iter()
            .all(|entry| entry.match_reason == WITHIN && entry.run.is_some()),
        "{:?}",
        plan.dropped
            .iter()
            .map(|entry| &entry.match_reason)
            .collect::<Vec<_>>()
    );
    let judged: HashSet<&str> = plan
        .dropped
        .iter()
        .filter_map(|entry| entry.run.map(|run| run.id.as_str()))
        .collect();
    assert_eq!(
        judged.len(),
        ids.len(),
        "every run was judged against the bound"
    );
}

fn run(id: &str, day: u32, hour: u32, minute: u32, boss: &str) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some(CHANNEL.into()),
        week_start: week_of(day),
        datetime: local(10, day, hour, minute),
        bosses: vec![boss.into()],
        participants: vec![MY.into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn bounded(runs: &[&Run], day_ref: Option<&str>, time_ref: Option<&str>) -> Extraction {
    Extraction {
        amendments: runs
            .iter()
            .map(|run| Amendment {
                bosses: run.bosses.clone(),
                day_ref: day_ref.map(str::to_owned),
                time_ref: time_ref.map(str::to_owned),
                target_run_hint: Some(format!("#{}", run.id)),
                confidence: 0.9,
                evidence_message_ids: vec!["101".into()],
                ..Amendment::new(AmendmentKind::Move)
            })
            .collect(),
        summary: "invented bound".into(),
    }
}

fn oct(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

/// The one run that must move, and the day it moves to (time always TBD).
fn moved<'a>(plan: &Plan<'a>) -> (&'a str, Option<NaiveDate>) {
    assert_eq!(plan.planned.len(), 1, "{:?}", plan.planned);
    let entry = &plan.planned[0];
    assert_eq!(entry.kind(), AmendmentKind::Move);
    assert_eq!(entry.resolved.clock, None);
    assert_eq!(entry.resolved.at, None);
    assert!(entry.needs_answer());
    assert!(
        plan.dropped
            .iter()
            .all(|entry| entry.match_reason == WITHIN)
    );
    (entry.run.expect("matched").id.as_str(), entry.resolved.day)
}

#[test]
fn a_run_before_the_bound_moves_to_its_day_with_the_time_tbd() {
    let early = run(
        "aaaaaaaa-0000-4000-8000-000000000001",
        16,
        22,
        0,
        "HCarling",
    );
    let late = run("bbbbbbbb-0000-4000-8000-000000000002", 17, 22, 0, "XKalos");
    let plan = plan_at(
        &bounded(&[&early, &late], Some("17th"), Some("after 0930")),
        &[&early, &late],
        sent(),
    );
    assert_eq!(moved(&plan), (early.id.as_str(), Some(oct(17))));
    assert_eq!(plan.dropped.len(), 1);
}

#[test]
fn after_a_day_alone_counts_from_that_days_midnight() {
    let early = run(
        "aaaaaaaa-0000-4000-8000-000000000001",
        16,
        23,
        30,
        "HCarling",
    );
    let same_day = run("bbbbbbbb-0000-4000-8000-000000000002", 17, 0, 30, "XKalos");
    let plan = plan_at(
        &bounded(&[&early, &same_day], Some("after 17th"), None),
        &[&early, &same_day],
        sent(),
    );
    assert_eq!(moved(&plan), (early.id.as_str(), Some(oct(17))));
}

#[test]
fn after_a_time_alone_applies_on_each_runs_own_day() {
    let early = run(
        "aaaaaaaa-0000-4000-8000-000000000001",
        17,
        21,
        0,
        "HCarling",
    );
    let late = run("bbbbbbbb-0000-4000-8000-000000000002", 16, 22, 30, "XKalos");
    let plan = plan_at(
        &bounded(&[&early, &late], None, Some("after 10")),
        &[&early, &late],
        sent(),
    );
    // No day was said, so none is proposed.
    assert_eq!(moved(&plan), (early.id.as_str(), None));
}

#[test]
fn before_is_strict_and_a_bare_day_offers_no_day() {
    let early = run(
        "aaaaaaaa-0000-4000-8000-000000000001",
        16,
        22,
        0,
        "HCarling",
    );
    let same_day = run("bbbbbbbb-0000-4000-8000-000000000002", 17, 0, 0, "XKalos");
    let plan = plan_at(
        &bounded(&[&early, &same_day], Some("before 17th"), None),
        &[&early, &same_day],
        sent(),
    );
    // Nothing on the 17th is before the 17th.
    assert_eq!(moved(&plan), (same_day.id.as_str(), None));

    let at_bound = run("cccccccc-0000-4000-8000-000000000003", 17, 23, 0, "HLimbo");
    let plan = plan_at(
        &bounded(&[&early, &at_bound], Some("17th"), Some("before 2300")),
        &[&early, &at_bound],
        sent(),
    );
    assert_eq!(moved(&plan), (at_bound.id.as_str(), Some(oct(17))));
}

#[test]
fn a_move_to_a_leading_zero_time_on_a_date_is_that_morning() {
    let night = run(
        "aaaaaaaa-0000-4000-8000-000000000001",
        17,
        22,
        0,
        "HCarling",
    );
    let plan = plan_at(
        &bounded(&[&night], Some("17th"), Some("0930")),
        &[&night],
        sent(),
    );
    assert_eq!(plan.planned.len(), 1);
    let at = plan.planned[0].resolved.at.expect("a slot").to_fixed();
    assert_eq!(at, local(10, 17, 9, 30));
}
