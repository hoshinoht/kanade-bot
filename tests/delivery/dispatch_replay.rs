//! Replays `dispatch.json` through the delivery executor, the real journal
//! (memory and SQLite) and the fake Discord.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use kanade::bot::delivery::{AlertRecorder, Delivery};
use kanade::bot::transport::{AmbiguousKind, Op, Step};
use kanade::domain::notify::due_reminders;
use kanade::domain::schedule::{NewRun, RsvpSource, RsvpState, RunSource, RunStatus};
use kanade::domain::scheduler::SchedulerError;
use kanade::domain::time::isoformat;
use serde_json::{Value, json};

use crate::common::{
    self, Deviation, SeqIds, apply_deviations, assert_case, instant, run_ref, step_result, strings,
    text, utc,
};
use crate::support::{self, Store, on_both_stores};

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

/// The notify target's named deviation, unchanged (user decision,
/// home-channel fallback): v4 falls back to the post channel, then refuses
/// to bind there, so the ping expires unsent; v5 sends to the fallback
/// channel, binds there and warns.
fn home_channel_fallback() -> Vec<Deviation> {
    let refused = "raised:DeliveryBindingError";
    let v4_attempts = [
        reminder_effect("555", &["1001"], refused, GONE_COUNTDOWN),
        reminder_effect("555", &["1001"], refused, GONE_DAY_OF),
        reminder_effect("555", &["1002"], refused, HOMELESS_DAY_OF),
    ];
    let mut deviations = vec![
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
    deviations.extend(sent(0, "700000000000000001"));
    deviations.extend(sent(1, "700000000000000002"));
    deviations.extend(sent(2, "700000000000000003"));
    deviations
}

async fn replay<S: Store>(store: &S, case: &Value) -> Value {
    let input = &case["input"];
    let mut now: DateTime<Utc> = utc(&input["clock"]);
    let mut ids = SeqIds::new(&input["uuid_sequence"]);
    let roster = support::roster(input);
    let channels = support::channels(input);
    let fake = support::fake();
    let alerts = AlertRecorder::new();
    let mut delivery = Delivery::new(
        store,
        ids.clone(),
        &fake,
        &alerts,
        &roster,
        &channels,
        support::config(input),
    );
    let mut effects = Vec::new();
    let mut run_refs = BTreeMap::new();
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let result: Result<Value, SchedulerError> = match text(&step["op"]) {
            "create_run" => {
                let id = support::service(store, &mut ids, now)
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
                support::service(store, &mut ids, now)
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .set_rsvp(&run, text(&step["user_id"]), state, source)
                    .await
                    .map(|()| json!(state.as_str()))
            }
            "add_reminder" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                support::service(store, &mut ids, now)
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_reminder(&run, text(&step["kind"]), utc(&step["fire_at"]), None)
                    .await
                    .map(|id| json!(id))
            }
            "due_reminders" => {
                let schedule = support::snapshot(store).await;
                let due = due_reminders(&schedule.reminders, now);
                Ok(json!(
                    due.iter()
                        .map(|row| json!({ "id": row.id, "kind": row.kind }))
                        .collect::<Vec<_>>()
                ))
            }
            "dispatch_reminders" => {
                let report = delivery.dispatch_reminders(now).await.expect("dispatch");
                assert_eq!(report.deferred, 0);
                effects.extend(report.sends.iter().map(support::effect_json));
                Ok(Value::Null)
            }
            "mark_done" => Ok(json!(delivery.mark_done(now).await.expect("mark_done"))),
            "set_clock" => {
                let at = instant(&step["clock"]);
                now = at.with_timezone(&Utc);
                Ok(json!(isoformat(&at)))
            }
            "set_transport" => {
                let fail = step["fail_sends"].as_bool().expect("fail_sends");
                let step = fail.then_some(Step::Ambiguous {
                    kind: AmbiguousKind::Timeout,
                    applied: false,
                });
                fake.set_default(Op::Create, step);
                Ok(json!(fail))
            }
            other => panic!("unknown dispatch vector operation {other:?}"),
        };
        steps.push(step_result(step, result));
    }
    let mut state = common::final_state(&support::snapshot(store).await, true);
    state["side_effects"] = json!(effects);
    json!({ "steps": steps, "final_state": state })
}

async fn every_case<S: Store>(store: &S, case: &Value) {
    let deviations = home_channel_fallback();
    let case_id = text(&case["case_id"]);
    let (expected, _) = apply_deviations(case_id, &case["expected"], &deviations);
    let actual = replay(store, case).await;
    assert_case(case_id, &actual, &expected);
}

#[tokio::test]
async fn every_dispatch_case_replays_on_both_stores() {
    let file = common::load("dispatch.json");
    assert_eq!(file["schema_version"], "v5-scheduler-dispatch-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let deviations = home_channel_fallback();
    let applied: usize = cases
        .iter()
        .map(|case| apply_deviations(text(&case["case_id"]), &case["expected"], &deviations).1)
        .sum();
    assert_eq!(applied, deviations.len(), "unused deviations");
    for case in cases {
        on_both_stores!(every_case, case);
    }
}
