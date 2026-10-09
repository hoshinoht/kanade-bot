//! Replays `digest.json` through the delivery tick's digest step, the real
//! journal (memory and SQLite) and the fake Discord.
//!
//! The monotone-week deviation (a clock behind the last posted week posts
//! nothing) changes no frozen case: none moves the clock backwards. It is
//! covered by `scenarios::clock_rollback_posts_nothing`.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use kanade::bot::delivery::{AlertRecorder, Delivery};
use kanade::bot::transport::{AmbiguousKind, Op, Step};
use kanade::domain::schedule::{NewFixedRun, NewRun, RunSource, RunStatus};
use kanade::domain::scheduler::SchedulerError;
use kanade::domain::time::{isoformat, to_iso};
use serde_json::{Value, json};

use crate::common::{
    self, SeqIds, assert_case, clock_time, instant, iso, opt_iso, run_ref, step_result, strings,
    text, utc, weekday,
};
use crate::support::{self, Store, on_both_stores, with_lease};

const LAST_DIGEST: &str = "last_digest_week";
const LAST_WEEK: &str = "last_materialised_week";

async fn replay<S: Store>(store: &S, case: &Value) -> Value {
    let input = &case["input"];
    let mut now: DateTime<Utc> = utc(&input["clock"]);
    let mut ids = SeqIds::new(&input["uuid_sequence"]);
    let roster = support::roster(input);
    let channels = support::channels(input);
    let fake = support::fake();
    let alerts = AlertRecorder::new();
    let config = support::config(input);
    let policy = config.policy.clone();
    let mut delivery = Delivery::new(
        store,
        ids.clone(),
        &fake,
        &alerts,
        &roster,
        &channels,
        config,
    );
    // v4 kept this in config; v5 keeps it per process (see `Delivery`).
    let mut last_materialised: Option<String> = None;
    let mut effects = Vec::new();
    let mut fixed_refs = BTreeMap::new();
    let mut run_refs = BTreeMap::new();
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let result: Result<Value, SchedulerError> = match text(&step["op"]) {
            "add_fixed" => {
                let id = support::service(store, &mut ids, now)
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_fixed_run(NewFixedRun {
                        owner_pinned: false,
                        owner_id: text(&step["owner_id"]).into(),
                        channel_id: step["channel_id"].as_str().map(str::to_owned),
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
            "set_status" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let status = RunStatus::parse(text(&step["status"])).unwrap();
                support::service(store, &mut ids, now)
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .set_run_status(&run, status)
                    .await
                    .map(|()| json!(status.as_str()))
            }
            "materialise_weeks" => {
                delivery.materialise(now).await.expect("materialise");
                let starts = policy.materialised_weeks(now).expect("in range");
                for start in &starts {
                    let week = start.to_fixed().with_timezone(&Utc);
                    let label = isoformat(&start.to_fixed());
                    support::register_materialised(store, &fixed_refs, week, &label, &mut run_refs)
                        .await;
                }
                last_materialised = Some(to_iso(&starts[0].to_fixed()).expect("in range"));
                Ok(Value::Null)
            }
            "post_week_digest" => {
                let report = delivery.post_week_digest(now).await.expect("digest");
                effects.extend(report.send.iter().map(support::effect_json));
                Ok(json!(report.message_id()))
            }
            "set_post_channel" => {
                delivery.config.post_channel_id = step["channel_id"].as_str().map(str::to_owned);
                Ok(step["channel_id"].clone())
            }
            "set_config" => {
                let value = text(&step["value"]).to_owned();
                match text(&step["key"]) {
                    LAST_DIGEST => {
                        let week = utc(&step["value"]);
                        with_lease(store, now, async |lease| {
                            store
                                .record_digest_week(lease, week, now)
                                .await
                                .expect("record week");
                        })
                        .await;
                    }
                    LAST_WEEK => last_materialised = Some(value.clone()),
                    other => panic!("unknown config key {other:?}"),
                }
                Ok(json!(value))
            }
            "set_weekly_digest" => {
                let week = utc(&step["week_start"]);
                support::seed_digest(
                    store,
                    week,
                    text(&step["channel_id"]),
                    text(&step["message_id"]),
                    now,
                )
                .await;
                fake.seed_message(
                    kanade::bot::ids::parse_id(text(&step["channel_id"])).unwrap(),
                    kanade::bot::ids::parse_id(text(&step["message_id"])).unwrap(),
                );
                Ok(Value::Null)
            }
            "retire_weekly_digests_before" => {
                let week = utc(&step["week_start"]);
                let retired = with_lease(store, now, async |lease| {
                    store
                        .retire_digests_before(lease, week, now)
                        .await
                        .expect("retire")
                })
                .await;
                Ok(json!(retired))
            }
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
            other => panic!("unknown digest vector operation {other:?}"),
        };
        steps.push(step_result(step, result));
    }
    let log = store.load_digests().await.expect("digests");
    let mut digests = log.digests;
    digests.sort_by_key(|row| row.week_start);
    let mut state = common::final_state(&support::snapshot(store).await, true);
    state["side_effects"] = json!(effects);
    state["weekly_digests"] = digests
        .iter()
        .map(|row| {
            json!({
                "week_start": iso(row.week_start),
                "channel_id": row.channel_id,
                "message_id": row.message_id,
                "posted_at": iso(row.posted_at),
                "retired_at": opt_iso(row.retired_at),
            })
        })
        .collect();
    state["config"] = json!({ LAST_DIGEST: log.last_digest_week, LAST_WEEK: last_materialised });
    json!({ "steps": steps, "final_state": state })
}

async fn every_case<S: Store>(store: &S, case: &Value) {
    let case_id = text(&case["case_id"]);
    let actual = replay(store, case).await;
    let expected = &case["expected"];
    assert_case(case_id, &actual, expected);
    for table in ["weekly_digests", "config"] {
        assert_eq!(
            actual["final_state"][table], expected["final_state"][table],
            "{case_id}: final {table}"
        );
    }
}

#[tokio::test]
async fn every_digest_case_replays_on_both_stores() {
    let file = common::load("digest.json");
    assert_eq!(file["schema_version"], "v5-scheduler-digest-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    for case in cases {
        on_both_stores!(every_case, case);
    }
}
