//! Replays `dispatch.json`: rows live in the scheduler service, dispatch plans
//! come from the pure core, and a scripted journal executes them.

use std::collections::BTreeMap;

use kanade::domain::{
    members::Roster,
    notify::{
        DeliverySettings, DeliveryTarget, DispatchInput, DispatchPlan, due_reminders, plan_dispatch,
    },
    schedule::{NewRun, RsvpSource, RsvpState, RunSource, RunStatus},
    scheduler::{Clock, SchedulerError},
    time::isoformat,
};
use serde_json::{Value, json};

use crate::{
    common::{
        self, Deviation, Service, assert_case, instant, run_ref, step_result, strings, text, utc,
    },
    harness::{self, Journal, Outcome},
    invariants,
};

fn reminder_effect(channel: &str, mentions: &[&str], outcome: &str, target: &str) -> Value {
    json!({
        "channel_id": channel,
        "dedupe_scope": "native",
        "effect_kind": "reminder",
        "mention_everyone": false,
        "mentions": mentions,
        "outcome": outcome,
        "role_mentions": [],
        "targets": [{"binding_type": "reminder", "key_primary": target, "key_secondary": null}],
    })
}

const FALLBACK_CASE: &str = "unavailable-home-falls-back-then-binding-refuses";
const GONE_RUN: &str = "00000001-0000-4000-8004-000000000000";
const HOMELESS_RUN: &str = "00000002-0000-4000-8004-000000000000";
const GONE_DAY_OF: &str = "00000003-0000-4000-8004-000000000000";
const GONE_COUNTDOWN: &str = "00000004-0000-4000-8004-000000000000";
const HOMELESS_DAY_OF: &str = "00000005-0000-4000-8004-000000000000";
const FIRST_TICK: &str = "2026-08-31T01:00:30+00:00";
const STALE_TICK: &str = "2026-08-31T13:00:01+00:00";

fn fallback(mentions: &[&str], target: &str, home: Option<&str>, run: &str) -> Value {
    let mut effect = reminder_effect("555", mentions, "bound", target);
    effect["warnings"] = json!([{
        "kind": "home_channel_unavailable",
        "home_channel_id": home,
        "run_ids": [run],
    }]);
    effect
}

fn sent(row: usize, message_id: &str) -> [Deviation; 2] {
    let field = |name: &str| format!("/final_state/reminders/{row}/{name}");
    [
        Deviation {
            case_id: FALLBACK_CASE,
            pointer: field("sent_at"),
            v4: json!(STALE_TICK),
            v5: json!(FIRST_TICK),
        },
        Deviation {
            case_id: FALLBACK_CASE,
            pointer: field("message_id"),
            v4: Value::Null,
            v5: json!(message_id),
        },
    ]
}

/// User decision (home-channel fallback): v4 falls back to `POST_CHANNEL_ID`,
/// then its journal refuses to bind a reminder to a channel other than the
/// run's, so the ping retries until stale and expires unsent. v5 sends to the
/// fallback channel, binds there, and warns that the home channel is broken.
fn home_channel_fallback() -> Vec<Deviation> {
    let refused = "raised:DeliveryBindingError";
    let v4_attempts = [
        reminder_effect("555", &["1001"], refused, GONE_COUNTDOWN),
        reminder_effect("555", &["1001"], refused, GONE_DAY_OF),
        reminder_effect("555", &["1002"], refused, HOMELESS_DAY_OF),
    ];
    let mut deviations = vec![
        // The `due_reminders` right after the first dispatch.
        Deviation {
            case_id: FALLBACK_CASE,
            pointer: "/steps/6/value".into(),
            v4: json!([
                {"id": GONE_DAY_OF, "kind": "day_of"},
                {"id": GONE_COUNTDOWN, "kind": "countdown_60"},
                {"id": HOMELESS_DAY_OF, "kind": "day_of"},
            ]),
            v5: json!([]),
        },
        Deviation {
            case_id: FALLBACK_CASE,
            pointer: "/final_state/side_effects".into(),
            v4: json!([v4_attempts.clone(), v4_attempts].concat()),
            v5: json!([
                fallback(&["1001"], GONE_COUNTDOWN, Some("444"), GONE_RUN),
                fallback(&["1001"], GONE_DAY_OF, Some("444"), GONE_RUN),
                fallback(&["1002"], HOMELESS_DAY_OF, None, HOMELESS_RUN),
            ]),
        },
    ];
    // Final rows: gone countdown_60, gone day_of, homeless day_of.
    deviations.extend(sent(0, "700000000000000001"));
    deviations.extend(sent(1, "700000000000000002"));
    deviations.extend(sent(2, "700000000000000003"));
    deviations
}

async fn dispatch(
    service: &mut Service,
    roster: &Roster,
    channels: &std::collections::BTreeSet<String>,
    post_channel: Option<&str>,
    journal: &mut Journal,
) {
    let schedule = common::snapshot(service).await;
    let held = journal.held.clone();
    let input = DispatchInput {
        now: service.clock().now(),
        schedule: &schedule,
        members: roster,
        channels,
        journal: &held,
        settings: DeliverySettings {
            post_channel_id: post_channel,
            quiet_mode: false,
            attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
        },
    };
    let plan = plan_dispatch(&input);
    invariants::check_dispatch_plan(&input, &plan);
    apply(service, journal, &plan).await;
    let after = common::snapshot(service).await;
    let held = journal.held.clone();
    let again = plan_dispatch(&DispatchInput {
        schedule: &after,
        journal: &held,
        ..input
    });
    invariants::check_nothing_replayed(&plan, &again);
}

async fn apply(service: &mut Service, journal: &mut Journal, plan: &DispatchPlan) {
    for retirement in &plan.retire {
        service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .mark_reminder_sent(&retirement.reminder_id, None)
            .await
            .expect("retire");
    }
    for send in &plan.sends {
        if let Outcome::Bound(message_id) = journal.execute(send) {
            for target in &send.intent.targets {
                let DeliveryTarget::Reminder(id) = target else {
                    panic!("dispatch bound a non-reminder target {target:?}");
                };
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .mark_reminder_sent(id, Some(&message_id))
                    .await
                    .expect("bind");
            }
        }
    }
}

async fn replay(case: &Value) -> Value {
    let input = &case["input"];
    let (mut service, clock) = common::service(input);
    let roster = harness::roster(input);
    let channels = harness::channels(input);
    let post_channel = input["post_channel_id"].as_str();
    let mut journal = Journal::new();
    let mut run_refs = BTreeMap::new();
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let result: Result<Value, SchedulerError> = match text(&step["op"]) {
            "create_run" => {
                let id = service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .create_run(NewRun {
                        fixed_run_id: None,
                        channel_id: step["channel_id"].as_str().map(str::to_owned),
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
            "set_rsvp" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let state = RsvpState::parse(text(&step["state"])).unwrap();
                let source = RsvpSource::parse(text(&step["source"])).unwrap();
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .set_rsvp(&run, text(&step["user_id"]), state, source)
                    .await
                    .map(|()| json!(state.as_str()))
            }
            "add_reminder" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_reminder(&run, text(&step["kind"]), utc(&step["fire_at"]), None)
                    .await
                    .map(|id| json!(id))
            }
            "due_reminders" => {
                let schedule = common::snapshot(&service).await;
                let due = due_reminders(&schedule.reminders, service.clock().now());
                Ok(json!(
                    due.iter()
                        .map(|row| json!({ "id": row.id, "kind": row.kind }))
                        .collect::<Vec<_>>()
                ))
            }
            "dispatch_reminders" => {
                dispatch(&mut service, &roster, &channels, post_channel, &mut journal).await;
                Ok(Value::Null)
            }
            "mark_done" => service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .mark_done()
                .await
                .map(|ids| json!(ids)),
            "set_clock" => {
                let at = instant(&step["clock"]);
                clock.set(at);
                Ok(json!(isoformat(&at)))
            }
            "set_transport" => {
                journal.fail_sends = step["fail_sends"].as_bool().expect("fail_sends");
                Ok(json!(journal.fail_sends))
            }
            other => panic!("unknown dispatch vector operation {other:?}"),
        };
        steps.push(step_result(step, result));
    }
    let mut state = common::final_state(&common::snapshot(&service).await, true);
    state["side_effects"] = json!(journal.effects);
    json!({ "steps": steps, "final_state": state })
}

#[tokio::test]
async fn every_dispatch_case_replays_exactly_except_named_deviations() {
    let file = common::load("dispatch.json");
    assert_eq!(file["family"], "dispatch");
    assert_eq!(file["schema_version"], "v5-scheduler-dispatch-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let deviations = home_channel_fallback();
    let (mut replayed, mut applied) = (0, 0);
    for case in cases {
        let case_id = text(&case["case_id"]);
        let (expected, used) = common::apply_deviations(case_id, &case["expected"], &deviations);
        let actual = replay(case).await;
        assert_case(case_id, &actual, &expected);
        replayed += 1;
        applied += used;
    }
    assert_eq!(replayed, cases.len(), "skipped cases");
    assert_eq!(applied, deviations.len(), "unused deviations");
}
