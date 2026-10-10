//! D-EXTRACT-STALE-HINT / D-EXTRACT-WEEK-ANCHOR: invented fixtures only.

use std::sync::Arc;

use chrono::{DateTime, NaiveTime, Utc, Weekday};
use kanade::domain::history::Origin;
use kanade::domain::model_log::{ModelLogStore, RescanStatus};
use kanade::domain::schedule::{NewRun, RsvpState, Run, RunSource, RunStatus};
use kanade::domain::scheduler::{ScheduleStore, Scope};
use kanade::extract::matching::{TERMINAL_HINT, match_run};
use kanade::extract::plan::{BurstInputs, Plan, plan_burst};
use kanade::extract::rescan::{RescanRequest, Rescans};
use kanade::extract::schema::{Extraction, parse_response};
use kanade::extract::{Amendment, AmendmentKind};

use crate::fakes::{
    ALVIN, CHANNEL, FakeHistory, KANON, MY, PRIYA, World, after, local, message, reply, zone,
};

fn run(id: &str, week: DateTime<Utc>, boss: &str) -> Run {
    Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some(CHANNEL.into()),
        week_start: week,
        datetime: week + chrono::TimeDelta::days(4),
        bosses: vec![boss.into()],
        participants: vec![MY.into()],
        status: RunStatus::Planned,
        source: RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    }
}

fn answer(kind: AmendmentKind) -> Amendment {
    Amendment {
        participants: vec![MY.into()],
        rsvp: Some(RsvpState::No),
        confidence: 0.9,
        evidence_message_ids: vec!["101".into()],
        ..Amendment::new(kind)
    }
}

fn plan<'a>(
    amendment: Amendment,
    channel: &[&'a Run],
    guild: &[&'a Run],
    evidence: &[DateTime<Utc>],
    anchor: DateTime<Utc>,
    reset_time: NaiveTime,
) -> Plan<'a> {
    let order: Vec<String> = (101..)
        .zip(evidence)
        .map(|(id, _)| id.to_string())
        .collect();
    let times = order
        .iter()
        .cloned()
        .zip(evidence.iter().copied())
        .collect();
    let authors = order.iter().cloned().map(|id| (id, MY.into())).collect();
    plan_burst(
        &Extraction {
            amendments: vec![amendment],
            summary: "invented answer".into(),
        },
        &BurstInputs {
            anchor,
            now: anchor,
            zone: zone(),
            reset_weekday: Weekday::Thu,
            reset_time,
            channel_runs: channel,
            guild_runs: guild,
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

#[test]
fn terminal_hints_refuse_before_scoring_week_filtering_and_spanning() {
    let week = local(8, 27, 0, 0);
    let anchor = local(8, 30, 13, 0);
    let live = run("bbbbbbbb", week, "HCarling");
    let other = run("cccccccc", week, "HMaleficStar");
    let next = run("dddddddd", local(9, 3, 0, 0), "HCarling");
    for status in [RunStatus::Done, RunStatus::Cancelled] {
        // Outside the anchored week, and bosses deliberately disagree.
        let terminal = Run {
            status,
            ..run("aaaaaaaa", local(8, 20, 0, 0), "XKalos")
        };
        for kind in [
            AmendmentKind::Rsvp,
            AmendmentKind::Sub,
            AmendmentKind::Move,
            AmendmentKind::Cancel,
            AmendmentKind::Otot,
            AmendmentKind::Split,
        ] {
            for guild_only in [false, true] {
                let mut amendment = answer(kind);
                amendment.target_run_hint = Some("#aaaa".into());
                amendment.bosses = vec!["HCarling".into(), "HMaleficStar".into()];
                amendment.is_question = true;
                let channel = if guild_only {
                    vec![&live, &other, &next]
                } else {
                    vec![&terminal, &live, &other, &next]
                };
                let result = plan(
                    amendment,
                    &channel,
                    &[&terminal],
                    &[anchor],
                    anchor,
                    NaiveTime::MIN,
                );
                assert!(result.planned.is_empty(), "{status:?} {kind:?}");
                assert_eq!(result.dropped.len(), 1);
                assert_eq!(result.dropped[0].match_code, TERMINAL_HINT);
                assert!(result.dropped[0].run.is_none());
            }
        }
    }
}

#[test]
fn unknown_hints_keep_current_scoring() {
    let anchor = local(8, 30, 13, 0);
    let live = run("bbbbbbbb", local(8, 27, 0, 0), "HCarling");
    let mut amendment = answer(AmendmentKind::Rsvp);
    amendment.target_run_hint = Some("#ffff".into());
    let result = plan(amendment, &[&live], &[], &[anchor], anchor, NaiveTime::MIN);
    assert_eq!(result.planned.len(), 1);
    assert_eq!(result.planned[0].run, Some(&live));
}

#[test]
fn dayless_answers_use_cited_evidence_week_at_the_configured_reset() {
    let reset = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let current = Run {
        datetime: local(9, 3, 5, 30),
        ..run("aaaaaaaa", local(8, 27, 6, 0), "XKalos")
    };
    let next = run("bbbbbbbb", local(9, 3, 6, 0), "HCarling");
    let previous = run("cccccccc", local(8, 20, 6, 0), "HCarling");
    let anchor = local(9, 3, 6, 1);
    for kind in [AmendmentKind::Rsvp, AmendmentKind::Sub] {
        for (evidence, expected) in [(local(9, 3, 5, 59), &current), (local(9, 3, 6, 0), &next)] {
            // Unrelated later burst text must not advance this answer's week.
            let result = plan(
                answer(kind),
                &[&next, &previous, &current],
                &[],
                &[evidence, anchor],
                anchor,
                reset,
            );
            assert_eq!(result.planned.len(), 1);
            assert_eq!(result.planned[0].run, Some(expected));
            // The same bound applies when the channel falls back guild-wide.
            let result = plan(
                answer(kind),
                &[],
                &[&next, &previous, &current],
                &[evidence, anchor],
                anchor,
                reset,
            );
            assert_eq!(result.planned[0].run, Some(expected));
        }
    }
    let result = plan(
        answer(AmendmentKind::Rsvp),
        &[&next],
        &[],
        &[local(9, 3, 5, 59)],
        anchor,
        reset,
    );
    assert!(result.planned.is_empty());
    assert_eq!(result.dropped.len(), 1);
}

#[test]
fn latest_known_citation_anchors_merged_evidence() {
    let current = run("aaaaaaaa", local(8, 27, 0, 0), "XKalos");
    let next = run("bbbbbbbb", local(9, 3, 0, 0), "HCarling");
    let anchor = local(9, 3, 0, 1);
    let mut amendment = answer(AmendmentKind::Rsvp);
    amendment.evidence_message_ids = vec!["102".into(), "unknown".into(), "101".into()];
    let result = plan(
        amendment,
        &[&current, &next],
        &[],
        &[local(9, 2, 23, 59), anchor],
        anchor,
        NaiveTime::MIN,
    );
    assert_eq!(result.planned[0].run, Some(&next));
    let result = plan(
        answer(AmendmentKind::Rsvp),
        &[&current, &next],
        &[],
        &[],
        anchor,
        NaiveTime::MIN,
    );
    assert_eq!(
        result.planned[0].run,
        Some(&next),
        "unknown evidence uses burst anchor"
    );
}

#[test]
fn rsvp_and_sub_ties_within_the_anchored_week_still_act() {
    let anchor = local(8, 30, 13, 0);
    let first = run("aaaaaaaa", local(8, 27, 0, 0), "XKalos");
    let second = run("bbbbbbbb", first.week_start, "HCarling");
    let next = run("cccccccc", local(9, 3, 0, 0), "HCarling");
    for kind in [AmendmentKind::Rsvp, AmendmentKind::Sub] {
        let result = plan(
            answer(kind),
            &[&next, &first, &second],
            &[],
            &[anchor],
            anchor,
            NaiveTime::MIN,
        );
        assert_eq!(result.planned.len(), 1);
        assert!(result.dropped.is_empty());
        assert!(result.planned[0].ambiguous);
        assert_eq!(result.planned[0].match_reason, "2 runs match equally well");
        assert_eq!(result.planned[0].run, Some(&first));
    }
}

#[test]
fn a_bare_clock_keeps_its_implicit_resolved_day() {
    let reset = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let current = Run {
        datetime: local(9, 3, 5, 30),
        ..run("aaaaaaaa", local(8, 27, 6, 0), "XKalos")
    };
    let next = run("bbbbbbbb", local(9, 3, 6, 0), "HCarling");
    let anchor = local(9, 3, 6, 1);
    for kind in [AmendmentKind::Rsvp, AmendmentKind::Sub] {
        let mut amendment = answer(kind);
        amendment.time_ref = Some("9pm".into());
        let result = plan(
            amendment,
            &[&next, &current],
            &[],
            &[local(9, 3, 5, 59)],
            anchor,
            reset,
        );
        assert_eq!(result.planned.len(), 1);
        assert_eq!(result.planned[0].run, Some(&next));
        assert_eq!(
            result.planned[0].resolved.day,
            Some(anchor.with_timezone(&zone()).date_naive())
        );
    }
}

#[test]
fn a_pre_reset_bare_clock_move_reaches_the_post_reset_run() {
    let reset = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
    let next = run("bbbbbbbb", local(9, 3, 6, 0), "HCarling");
    let anchor = local(9, 3, 5, 59);
    let mut amendment = answer(AmendmentKind::Move);
    amendment.time_ref = Some("9pm".into());
    let result = plan(amendment, &[&next], &[], &[anchor], anchor, reset);
    assert_eq!(result.planned.len(), 1);
    assert_eq!(result.planned[0].run, Some(&next));
    assert_eq!(
        result.planned[0]
            .resolved
            .at
            .unwrap()
            .to_fixed()
            .with_timezone(&Utc),
        local(9, 7, 21, 0)
    );
}

#[test]
fn other_dayless_run_acting_kinds_keep_v4_reachability() {
    let anchor = local(8, 30, 13, 0);
    let next = run("bbbbbbbb", local(9, 3, 0, 0), "HCarling");
    for kind in [
        AmendmentKind::Move,
        AmendmentKind::Cancel,
        AmendmentKind::Otot,
        AmendmentKind::Split,
    ] {
        let result = plan(
            answer(kind),
            &[&next],
            &[],
            &[anchor],
            anchor,
            NaiveTime::MIN,
        );
        assert_eq!(result.planned.len(), 1, "{kind:?}");
        assert_eq!(result.planned[0].run, Some(&next));
    }
}

#[test]
fn add_and_fix_hints_at_done_runs_do_not_refuse() {
    let anchor = local(8, 30, 13, 0);
    let terminal = Run {
        status: RunStatus::Done,
        ..run("aaaaaaaa", local(8, 27, 0, 0), "XKalos")
    };
    for kind in [AmendmentKind::Add, AmendmentKind::Fix] {
        let mut amendment = answer(kind);
        amendment.target_run_hint = Some("#aaaa".into());
        amendment.bosses = vec!["HCarling".into()];
        amendment.day_ref = Some("2026-09-07".into());
        amendment.time_ref = Some("9pm".into());
        let result = plan(
            amendment,
            &[&terminal],
            &[],
            &[anchor],
            anchor,
            NaiveTime::MIN,
        );
        assert!(result.dropped.is_empty());
        assert_eq!(result.planned.len(), 1);
        assert!(result.planned[0].run.is_none());
        assert_ne!(result.planned[0].match_code, TERMINAL_HINT);
    }
}

#[test]
fn a_hint_prefix_matching_both_done_and_live_runs_keeps_the_live_match() {
    let week = local(8, 27, 0, 0);
    let terminal = Run {
        status: RunStatus::Done,
        ..run("aaaa0000", week, "XKalos")
    };
    let live = run("aaaa1111", week, "HCarling");
    let mut amendment = answer(AmendmentKind::Rsvp);
    amendment.target_run_hint = Some("#aaaa".into());
    let result = match_run(
        &amendment,
        &[&terminal, &live],
        &[],
        Some(MY),
        &[] as &[String],
    );
    assert_eq!(result.run, Some(&live));
    assert_eq!(result.reason, "model pointed at #aaaa1111");
    let anchor = local(8, 30, 13, 0);
    let result = plan(
        amendment,
        &[&terminal, &live],
        &[],
        &[anchor],
        anchor,
        NaiveTime::MIN,
    );
    assert_eq!(result.planned[0].run, Some(&live));
}

#[test]
fn anchored_answers_do_not_escape_a_channel_with_live_runs_in_another_week() {
    let anchor = local(8, 30, 13, 0);
    let terminal = Run {
        status: RunStatus::Done,
        ..run("aaaaaaaa", local(8, 27, 0, 0), "XKalos")
    };
    let next = run("bbbbbbbb", local(9, 3, 0, 0), "HCarling");
    let elsewhere = Run {
        channel_id: Some("901".into()),
        ..run("cccccccc", terminal.week_start, "HCarling")
    };
    for kind in [AmendmentKind::Rsvp, AmendmentKind::Sub] {
        let result = plan(
            answer(kind),
            &[&terminal, &next],
            &[&elsewhere],
            &[anchor],
            anchor,
            NaiveTime::MIN,
        );
        assert!(result.planned.is_empty());
        assert_eq!(result.dropped.len(), 1);
        assert!(result.dropped[0].run.is_none());
        let result = plan(
            answer(kind),
            &[&terminal],
            &[&elsewhere],
            &[anchor],
            anchor,
            NaiveTime::MIN,
        );
        assert_eq!(
            result.planned[0].run,
            Some(&elsewhere),
            "no live channel runs permits guild fallback"
        );
    }
    let mut dated = answer(AmendmentKind::Rsvp);
    dated.day_ref = Some("2026-09-02".into());
    let result = plan(
        dated,
        &[&terminal, &next],
        &[&elsewhere],
        &[anchor],
        anchor,
        NaiveTime::MIN,
    );
    assert_eq!(
        result.planned[0].run,
        Some(&elsewhere),
        "dated fallback retains v4 behaviour"
    );
}

#[test]
fn dayless_spanning_stays_inside_the_evidence_week() {
    let anchor = local(8, 30, 13, 0);
    let first = run("aaaaaaaa", local(8, 27, 0, 0), "XKalos");
    let second = run("bbbbbbbb", first.week_start, "HCarling");
    let next = run("cccccccc", local(9, 3, 0, 0), "HCarling");
    let mut amendment = answer(AmendmentKind::Sub);
    amendment.bosses = vec!["XKalos".into(), "HCarling".into()];
    let result = plan(
        amendment,
        &[&next, &first, &second],
        &[],
        &[anchor],
        anchor,
        NaiveTime::MIN,
    );
    assert_eq!(result.planned.len(), 2);
    assert!(
        result
            .planned
            .iter()
            .all(|entry| entry.run.unwrap().week_start == first.week_start)
    );
}

#[test]
fn dated_answers_keep_existing_reachability() {
    let anchor = local(8, 30, 13, 0);
    let next = run("bbbbbbbb", local(9, 3, 0, 0), "HCarling");
    let mut amendment = answer(AmendmentKind::Rsvp);
    amendment.day_ref = Some("2026-09-07".into());
    let result = plan(amendment, &[&next], &[], &[anchor], anchor, NaiveTime::MIN);
    assert_eq!(result.planned[0].run, Some(&next));
}

async fn next_run(world: &World) -> String {
    world
        .scheduler
        .service()
        .as_origin(Origin::for_tests())
        .create_run(NewRun {
            fixed_run_id: None,
            channel_id: Some(CHANNEL.into()),
            week_start: local(9, 3, 0, 0),
            datetime: local(9, 8, 22, 0),
            bosses: vec!["HCarling".into()],
            participants: vec![MY.into(), ALVIN.into(), KANON.into()],
            status: RunStatus::Planned,
            source: RunSource::Amend,
        })
        .await
        .expect("next week's live run")
}

#[tokio::test(start_paused = true)]
async fn startup_rescan_refuses_a_day_old_answer_hinting_a_done_run() {
    // World uses deterministic invented UUIDs; cite its second (XKalos) run.
    let hint = "0000d4af-0000-4000-8000-000000000002";
    let world = World::new(vec![reply(&format!(
        r##"{{"amendments":[{{"kind":"rsvp",
        "bosses":[],"day_ref":null,"target_run_hint":"#{hint}","participants":["{MY}"],
        "rsvp":"no","confidence":0.9,"evidence_message_ids":["101"]}}]}}"##
    ))])
    .await;
    assert_eq!(world.runs[1], hint);
    world.scheduler.clock.set(local(9, 2, 13, 0).fixed_offset());
    for id in &world.runs {
        world
            .scheduler
            .service()
            .as_origin(Origin::for_tests())
            .set_run_status(id, RunStatus::Done)
            .await
            .expect("done");
    }
    next_run(&world).await;
    world
        .extractor
        .store_message(&message("101", MY, local(9, 1, 13, 0), "XKalos no"))
        .await
        .expect("cache failed burst");
    let jobs = Arc::new(Rescans::new(
        world.extractor.clone(),
        Arc::new(FakeHistory::default()),
    ));
    let id = jobs
        .submit(RescanRequest {
            channels: vec![CHANNEL.into()],
            window: "48h".into(),
            source: "startup".into(),
            automated: true,
            requested_by: None,
            unprocessed_only: true,
        })
        .await
        .expect("startup rescan")
        .job
        .id;
    let worker = {
        let jobs = jobs.clone();
        tokio::spawn(async move { jobs.run().await })
    };
    after(1).await;
    let stored = world.store.load_rescan_job(&id).await.unwrap().unwrap();
    assert_eq!(stored.status, RescanStatus::Done);
    assert_eq!(stored.results[0]["dropped"], 1);
    assert_eq!(world.requests(), 1);
    // Replan the recorded reply against the same schedule snapshot to assert
    // the refusal itself, not merely a drop from the independent week anchor.
    let extraction = parse_response(&world.logs().await[0].raw_response).expect("recorded reply");
    let snapshot = world.store.load(&Scope::All).await.expect("schedule");
    let guild: Vec<_> = snapshot.runs.iter().collect();
    let channel: Vec<_> = guild
        .iter()
        .copied()
        .filter(|run| run.channel_id.as_deref() == Some(CHANNEL))
        .collect();
    let incident_plan = plan(
        extraction.amendments[0].clone(),
        &channel,
        &guild,
        &[local(9, 1, 13, 0)],
        local(9, 1, 13, 0),
        NaiveTime::MIN,
    );
    assert_eq!(incident_plan.dropped.len(), 1);
    assert_eq!(incident_plan.dropped[0].match_code, TERMINAL_HINT);
    assert!(
        world.outbox.answers.lock().unwrap().is_empty(),
        "no RSVP handed to reaction path"
    );
    assert!(world.outbox.cards.lock().unwrap().is_empty());
    assert!(world.live_proposals().await.is_empty());
    assert!(world.processed("101").await);
    jobs.close().await;
    worker.await.expect("worker joined");
}

#[tokio::test(start_paused = true)]
async fn pipeline_threads_each_cited_message_time_not_the_latest_burst_time() {
    let world = World::new(vec![reply(&format!(
        r#"{{"amendments":[{{"kind":"rsvp",
        "bosses":[],"day_ref":null,"participants":["{MY}"],"rsvp":"no",
        "confidence":0.9,"evidence_message_ids":["101"]}}]}}"#
    ))])
    .await;
    next_run(&world).await;
    world.scheduler.clock.set(local(9, 3, 0, 2).fixed_offset());
    let messages = [
        message("101", MY, local(9, 2, 23, 59), "XKalos no"),
        message("102", PRIYA, local(9, 3, 0, 1), "carling 9pm?"),
    ];
    let mut burst = Vec::new();
    for message in &messages {
        world.extractor.store_message(message).await.expect("cache");
        burst.push(kanade::extract::backlog::BacklogEntry {
            channel_id: CHANNEL.into(),
            message_id: message.id.clone(),
            created_at: message.created_at,
        });
    }
    let report = world.extractor.flush(CHANNEL, &burst).await;
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(world.requests(), 1);
    assert_eq!(report.dropped, 1);
    assert_eq!(report.answers, 0);
    assert!(world.outbox.answers.lock().unwrap().is_empty());
}
