//! Replays `reminders.json`: specs, rows, reconciliation, lifecycle and staleness.

use std::collections::BTreeMap;

use kanade::domain::{
    schedule::{
        NewFixedRun, NewRun, ReminderPolicy, RunSource, RunStatus, is_stale, reminder_specs,
    },
    scheduler::SchedulerError,
    time::isoformat,
};
use serde_json::{Value, json};

use crate::common::{
    self, Deviation, Service, apply_deviations, assert_case, clock_time, countdowns, instant, iso,
    opt_iso, register_materialised, run_ref, step_result, strings, text, utc, weekday, zone,
};

/// Decision 1 (DST spring-forward gap): v4 resolves a nonexistent `day_of`
/// wall clock with the pre-transition offset, which can fire at or after the
/// run start; v5 clamps such a ping to one second before the start.
fn dst_gap_day_of_clamp() -> Vec<Deviation> {
    const CASE: &str = "dst-spring-forward-gap-ping-and-fixed-slots";
    let entry = |pointer: &str, v4: &str, v5: &str| Deviation {
        case_id: CASE,
        pointer: pointer.into(),
        v4: json!(v4),
        v5: json!(v5),
    };
    vec![
        // Run 03:00 EDT, ping 02:30: v4 fired 30 min after the start.
        entry(
            "/steps/0/value/0/fire_at",
            "2026-03-08T02:30:00-05:00",
            "2026-03-08T01:59:59-05:00",
        ),
        // Materialised `after` run (03:00 EDT = 07:00Z): same as above, as a row.
        entry(
            "/final_state/reminders/2/fire_at",
            "2026-03-08T07:30:00+00:00",
            "2026-03-08T06:59:59+00:00",
        ),
        // Materialised `gap` run (02:30 slot = 07:30Z): v4 fired exactly at the start.
        entry(
            "/final_state/reminders/5/fire_at",
            "2026-03-08T07:30:00+00:00",
            "2026-03-08T07:29:59+00:00",
        ),
    ]
}

fn policy(zone: chrono_tz::Tz, step: &Value) -> ReminderPolicy {
    ReminderPolicy {
        zone,
        ping_time: clock_time(&step["ping_time"]),
        countdowns: step.get("countdowns").map(countdowns).unwrap_or_default(),
    }
}

async fn reminder_id(service: &Service, run: &str, kind: &str) -> String {
    service
        .reminders(run)
        .await
        .expect("reminders")
        .into_iter()
        .find(|row| row.kind == kind)
        .unwrap_or_else(|| panic!("run {run} has no {kind} reminder"))
        .id
}

async fn replay(case: &Value) -> Value {
    let input = &case["input"];
    let tz = zone(input);
    let (mut service, clock) = common::service(input);
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
                        week_start: utc(&step["week_start"]),
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
                let start = instant(&step["week_start"]);
                let created = service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .materialise_week(start, &policy(tz, step))
                    .await;
                let week = utc(&step["week_start"]);
                register_materialised(
                    &service,
                    &fixed_refs,
                    week,
                    text(&step["week_start"]),
                    &mut run_refs,
                )
                .await;
                created.map(|ids| json!(ids))
            }
            "set_status" => match RunStatus::parse(text(&step["status"])) {
                Err(error) => Err(error.into()),
                Ok(status) => {
                    let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                    service
                        .as_origin(kanade::domain::history::Origin::for_tests())
                        .set_run_status(&run, status)
                        .await
                        .map(|()| json!(status.as_str()))
                }
            },
            "set_clock" => {
                let at = instant(&step["clock"]);
                clock.set(at);
                Ok(json!(isoformat(&at)))
            }
            "reminder_specs" => {
                let status = RunStatus::parse(text(&step["status"])).unwrap();
                reminder_specs(instant(&step["at"]), status, &policy(tz, step))
                    .map(|specs| {
                        json!(
                            specs
                                .iter()
                                .map(|spec| json!({
                                    "kind": spec.kind,
                                    "fire_at": isoformat(&spec.fire_at),
                                }))
                                .collect::<Vec<_>>()
                        )
                    })
                    .map_err(Into::into)
            }
            "ensure_reminders" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let rebuild = step["rebuild"].as_bool().expect("rebuild");
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .ensure_reminders(&run, &policy(tz, step), rebuild)
                    .await
                    .map(|kinds| json!(kinds))
            }
            "add_reminder" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let sent_at = (!step["sent_at"].is_null()).then(|| utc(&step["sent_at"]));
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_reminder(&run, text(&step["kind"]), utc(&step["fire_at"]), sent_at)
                    .await
                    .map(|id| json!(id))
            }
            "mark_reminder_sent" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let id = reminder_id(&service, &run, text(&step["kind"])).await;
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .mark_reminder_sent(&id, step["message_id"].as_str())
                    .await
                    .map(|()| Value::Null)
            }
            "reschedule_unposted_reminder" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let id = reminder_id(&service, &run, text(&step["kind"])).await;
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .reschedule_unposted_reminder(&id, utc(&step["fire_at"]))
                    .await
                    .map(|changed| json!(changed))
            }
            "reconcile_day_of" => service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .reconcile_day_of(&policy(tz, step))
                .await
                .map(|count| json!(count)),
            "mark_done" => service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .mark_done()
                .await
                .map(|ids| json!(ids)),
            "list_reminders" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                service.reminders(&run).await.map(|rows| {
                    json!(
                        rows.iter()
                            .map(|row| json!({
                                "id": row.id,
                                "kind": row.kind,
                                "fire_at": iso(row.fire_at),
                                "sent_at": opt_iso(row.sent_at),
                                "message_id": row.message_id,
                            }))
                            .collect::<Vec<_>>()
                    )
                })
            }
            "is_stale" => Ok(json!(is_stale(
                text(&step["kind"]),
                utc(&step["fire_at"]),
                utc(&step["now"]),
            ))),
            other => panic!("unknown reminders vector operation {other:?}"),
        };
        steps.push(step_result(step, result));
    }
    json!({
        "steps": steps,
        "final_state": common::final_state(&common::snapshot(&service).await, true),
    })
}

#[tokio::test]
async fn every_reminder_case_replays_exactly_except_named_deviations() {
    let file = common::load("reminders.json");
    assert_eq!(file["family"], "reminders");
    assert_eq!(file["schema_version"], "v5-scheduler-reminders-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let deviations = dst_gap_day_of_clamp();
    let (mut replayed, mut used) = (0, 0);
    for case in cases {
        let case_id = text(&case["case_id"]);
        let (expected, applied) = apply_deviations(case_id, &case["expected"], &deviations);
        let actual = replay(case).await;
        assert_case(case_id, &actual, &expected);
        replayed += 1;
        used += applied;
    }
    assert_eq!(replayed, cases.len(), "skipped cases");
    assert_eq!(used, deviations.len(), "unused deviations");
}
