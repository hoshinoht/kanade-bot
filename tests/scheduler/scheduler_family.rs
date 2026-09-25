//! Replays `scheduler.json`: materialisation, adoption, fixed edit/retire and RSVPs.

use std::collections::BTreeMap;

use chrono::Utc;
use kanade::domain::{
    schedule::{
        FixedField, FixedRunPatch, NewFixedRun, NewRun, ReminderPolicy, RsvpSource, RsvpState,
        RunSource, RunStatus,
    },
    scheduler::SchedulerError,
    time::isoformat,
};
use serde_json::{Value, json};

use crate::common::{
    self, assert_case, clock_time, countdowns, instant, register_materialised, run_ref,
    step_result, strings, text, utc, weekday, zone,
};

async fn replay(case: &Value) -> Value {
    let input = &case["input"];
    let (mut service, _clock) = common::service(input);
    let policy = ReminderPolicy {
        zone: zone(input),
        ping_time: clock_time(&input["ping_time"]),
        countdowns: countdowns(&input["countdowns"]),
    };
    let default_week = &input["week_start"];
    let mut fixed_refs = BTreeMap::new();
    let mut run_refs = BTreeMap::new();
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let result: Result<Value, SchedulerError> = match text(&step["op"]) {
            "add_fixed" => {
                let id = service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_fixed_run(NewFixedRun {
                        owner_id: text(&step["owner_id"]).into(),
                        channel_id: Some(text(&step["channel_id"]).into()),
                        bosses: strings(&step["bosses"]),
                        weekday: weekday(&step["weekday"]),
                        time: clock_time(&step["time"]),
                        participants: strings(&step["participants"]),
                        note: None,
                    })
                    .await
                    .expect("add_fixed");
                fixed_refs.insert(text(&step["fixed_key"]).to_owned(), id.clone());
                Ok(json!(id))
            }
            "create_run" => {
                let id = service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .create_run(NewRun {
                        fixed_run_id: None,
                        channel_id: Some(text(&step["channel_id"]).into()),
                        week_start: utc(default_week),
                        datetime: utc(&step["at"]),
                        bosses: strings(&step["bosses"]),
                        participants: strings(&step["participants"]),
                        status: RunStatus::parse(text(&step["status"])).unwrap(),
                        source: RunSource::parse(text(&step["source"])).unwrap(),
                    })
                    .await
                    .expect("create_run");
                run_refs.insert(text(&step["run_key"]).to_owned(), id.clone());
                Ok(json!(id))
            }
            "materialise" => {
                let start = instant(step.get("week_start").unwrap_or(default_week));
                let created = service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .materialise_week(start, &policy)
                    .await;
                let week_text = isoformat(&start);
                let week = start.with_timezone(&Utc);
                register_materialised(&service, &fixed_refs, week, &week_text, &mut run_refs).await;
                created.map(|ids| json!(ids))
            }
            "set_status" => {
                let status = RunStatus::parse(text(&step["status"])).unwrap();
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .set_run_status(&run, status)
                    .await
                    .map(|()| json!(status.as_str()))
            }
            "set_rsvp" => match RsvpState::parse(text(&step["state"])) {
                Err(error) => Err(error.into()),
                Ok(state) => {
                    let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                    let source = RsvpSource::parse(text(&step["source"])).unwrap();
                    service
                        .as_origin(kanade::domain::history::Origin::for_tests())
                        .set_rsvp(&run, text(&step["user_id"]), state, source)
                        .await
                        .map(|()| json!(state.as_str()))
                }
            },
            "reaction" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let added = step["added"].as_bool().expect("added");
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .apply_reaction(&run, text(&step["user_id"]), text(&step["emoji"]), added)
                    .await
                    .map(|result| {
                        json!({
                            "run_id": result.run_id,
                            "applied": result.applied,
                            "state": result.state.map(RsvpState::as_str),
                            "old_status": result.old_status.as_str(),
                            "new_status": result.new_status.as_str(),
                        })
                    })
            }
            "edit_fixed" => {
                let fixed = fixed_refs[text(&step["fixed_key"])].clone();
                let fields = step["fields"].as_object().expect("fields");
                let mut patch = FixedRunPatch::default();
                for (key, value) in fields {
                    match key.as_str() {
                        "weekday" => patch.weekday = Some(weekday(value)),
                        "time" => patch.time = Some(clock_time(value)),
                        other => panic!("unsupported fixed field {other}"),
                    }
                }
                let changed: Vec<FixedField> = strings(&step["changed"])
                    .iter()
                    .map(|name| match name.as_str() {
                        "weekday" => FixedField::Weekday,
                        "time" => FixedField::Time,
                        other => panic!("unsupported changed field {other}"),
                    })
                    .collect();
                let weeks: Vec<_> = step["week_starts"]
                    .as_array()
                    .expect("week_starts")
                    .iter()
                    .map(instant)
                    .collect();
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .edit_fixed_run(&fixed, patch, &changed, &weeks, &policy)
                    .await
                    .map(|count| json!(count))
            }
            "retire_fixed" => {
                let fixed = fixed_refs
                    .remove(text(&step["fixed_key"]))
                    .expect("known fixed key");
                let weeks: Vec<_> = step["week_starts"]
                    .as_array()
                    .expect("week_starts")
                    .iter()
                    .map(instant)
                    .collect();
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .retire_fixed_run(&fixed, &weeks, &policy)
                    .await
                    .map(|count| json!(count))
            }
            other => panic!("unknown scheduler vector operation {other:?}"),
        };
        steps.push(step_result(step, result));
    }
    json!({
        "steps": steps,
        "final_state": common::final_state(&common::snapshot(&service).await, false),
    })
}

#[tokio::test]
async fn every_scheduler_case_replays_exactly() {
    let file = common::load("scheduler.json");
    assert_eq!(file["family"], "scheduler");
    assert_eq!(file["schema_version"], "v5-scheduler-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let mut replayed = 0;
    for case in cases {
        let case_id = text(&case["case_id"]);
        let actual = replay(case).await;
        assert_case(case_id, &actual, &case["expected"]);
        replayed += 1;
    }
    assert_eq!(replayed, cases.len(), "skipped cases");
}
