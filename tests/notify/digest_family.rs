//! Replays `digest.json`: the tick decision and post plan come from the pure
//! core; the test holds the config stamps and digest log a store would.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use kanade::domain::{
    notify::{
        DeliverySettings, DeliveryTarget, DigestAction, DigestPostInput, DigestSend, RecordReason,
        WeekReset, WeeklyDigest, plan_digest_post, plan_digest_tick, retire_digests_before,
    },
    schedule::{NewFixedRun, NewRun, ReminderPolicy, RunSource, RunStatus},
    scheduler::{Clock, SchedulerError},
    time::{isoformat, to_iso},
    weeks,
};
use serde_json::{Value, json};

use crate::{
    common::{
        self, Service, TestClock, assert_case, clock_time, countdowns, instant, iso, opt_iso,
        run_ref, step_result, strings, text, utc, weekday, zone,
    },
    harness::{self, Journal, Outcome},
};

const LAST_DIGEST: &str = "last_digest_week";
const LAST_WEEK: &str = "last_materialised_week";

/// What the store and host hold beyond scheduler rows.
struct Host {
    reset: WeekReset,
    policy: ReminderPolicy,
    post_channel: Option<String>,
    channels: std::collections::BTreeSet<String>,
    config: BTreeMap<&'static str, String>,
    digests: Vec<WeeklyDigest>,
    journal: Journal,
}

impl Host {
    fn new(input: &Value) -> Self {
        let tz = zone(input);
        Self {
            reset: WeekReset {
                zone: tz,
                weekday: weekday(&input["reset_weekday"]),
                time: clock_time(&input["reset_time"]),
            },
            policy: ReminderPolicy {
                zone: tz,
                ping_time: clock_time(&input["ping_time"]),
                countdowns: countdowns(&input["countdowns"]),
            },
            post_channel: input["post_channel_id"].as_str().map(str::to_owned),
            channels: harness::channels(input),
            config: BTreeMap::new(),
            digests: Vec::new(),
            journal: Journal::new(),
        }
    }

    fn retire_before(&mut self, week: DateTime<Utc>, now: DateTime<Utc>) -> usize {
        let weeks = retire_digests_before(&self.digests, week);
        for row in &mut self.digests {
            if weeks.contains(&row.week_start) {
                row.retired_at = Some(now);
            }
        }
        weeks.len()
    }

    /// v4 `Repo.set_weekly_digest`: upsert, reactivating the row.
    fn set_digest(
        &mut self,
        week: DateTime<Utc>,
        channel: &str,
        message: &str,
        now: DateTime<Utc>,
    ) {
        self.digests.retain(|row| row.week_start != week);
        self.digests.push(WeeklyDigest {
            week_start: week,
            channel_id: channel.to_owned(),
            message_id: message.to_owned(),
            posted_at: now,
            retired_at: None,
        });
        self.digests.sort_by_key(|row| row.week_start);
    }

    async fn post_week_digest(&mut self, service: &Service, now: DateTime<Utc>) -> Option<String> {
        let tick = plan_digest_tick(
            &self.reset,
            now,
            self.config.get(LAST_DIGEST).map(String::as_str),
            self.post_channel.as_deref(),
        )
        .expect("in range");
        self.retire_before(tick.retire_before, now);
        let current = to_iso(&tick.current_week).expect("in range");
        match tick.action {
            DigestAction::UpToDate => None,
            DigestAction::Record(RecordReason::NoPostChannel | RecordReason::FirstTick) => {
                self.config.insert(LAST_DIGEST, current);
                None
            }
            DigestAction::Post => {
                // v4 `_post_digest` retires again before choosing a channel.
                self.retire_before(tick.current_week, now);
                let runs = common::snapshot(service).await.runs;
                let held = self.journal.held.clone();
                let post = plan_digest_post(&DigestPostInput {
                    week_start: tick.current_week,
                    current_week: tick.current_week,
                    zone: self.reset.zone,
                    runs: &runs,
                    digests: &self.digests,
                    explicit_channel: None,
                    settings: DeliverySettings {
                        post_channel_id: self.post_channel.as_deref(),
                        quiet_mode: false,
                        attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
                    },
                    channels: &self.channels,
                    journal: &held,
                })
                .expect("in range");
                // No channel: nothing is stamped.
                let DigestSend {
                    send,
                    replaces,
                    record_week,
                } = post?;
                assert_eq!(replaces, None, "no vector replaces an active card");
                let Outcome::Bound(message) = self.journal.execute(&send) else {
                    return None;
                };
                let [DeliveryTarget::Digest(week)] = send.intent.targets.as_slice() else {
                    panic!("digest intent targets {:?}", send.intent.targets);
                };
                let channel = send.intent.channel_id.clone();
                self.set_digest(*week, &channel, &message, now);
                if let Some(week) = record_week {
                    self.config
                        .insert(LAST_DIGEST, to_iso(&week).expect("in range"));
                }
                Some(message)
            }
        }
    }

    /// v4 `BossBot.materialise_weeks`.
    async fn materialise_weeks(
        &mut self,
        service: &mut Service,
        now: DateTime<Utc>,
        fixed_refs: &BTreeMap<String, String>,
        run_refs: &mut BTreeMap<String, String>,
    ) {
        let starts = weeks::materialised_week_starts(
            self.reset.zone,
            self.reset.weekday,
            self.reset.time,
            &now,
        )
        .expect("in range");
        for start in &starts {
            service
                .as_origin(kanade::domain::history::Origin::for_tests())
                .materialise_week(start.to_fixed(), &self.policy)
                .await
                .expect("materialise");
        }
        service
            .as_origin(kanade::domain::history::Origin::for_tests())
            .reconcile_day_of(&self.policy)
            .await
            .expect("reconcile");
        for start in &starts {
            let week = start.to_fixed().with_timezone(&Utc);
            let text = isoformat(&start.to_fixed());
            common::register_materialised(service, fixed_refs, week, &text, run_refs).await;
        }
        let current = to_iso(&starts[0].to_fixed()).expect("in range");
        self.config.insert(LAST_WEEK, current);
    }

    fn state(&self) -> Value {
        json!({
            "weekly_digests": self.digests.iter().map(|row| json!({
                "week_start": iso(row.week_start),
                "channel_id": row.channel_id,
                "message_id": row.message_id,
                "posted_at": iso(row.posted_at),
                "retired_at": opt_iso(row.retired_at),
            })).collect::<Vec<_>>(),
            "config": {
                LAST_DIGEST: self.config.get(LAST_DIGEST),
                LAST_WEEK: self.config.get(LAST_WEEK),
            },
        })
    }
}

fn config_key(value: &Value) -> &'static str {
    match text(value) {
        LAST_DIGEST => LAST_DIGEST,
        LAST_WEEK => LAST_WEEK,
        other => panic!("unknown config key {other:?}"),
    }
}

async fn replay(case: &Value) -> Value {
    let input = &case["input"];
    let (mut service, clock): (Service, TestClock) = common::service(input);
    let mut host = Host::new(input);
    let mut fixed_refs = BTreeMap::new();
    let mut run_refs = BTreeMap::new();
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        let now = clock.now();
        let result: Result<Value, SchedulerError> = match text(&step["op"]) {
            "add_fixed" => {
                let id = service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_fixed_run(NewFixedRun {
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
            "set_status" => {
                let run = run_ref(&run_refs, &step["run_key"]).to_owned();
                let status = RunStatus::parse(text(&step["status"])).unwrap();
                service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .set_run_status(&run, status)
                    .await
                    .map(|()| json!(status.as_str()))
            }
            "materialise_weeks" => {
                host.materialise_weeks(&mut service, now, &fixed_refs, &mut run_refs)
                    .await;
                Ok(Value::Null)
            }
            "post_week_digest" => Ok(json!(host.post_week_digest(&service, now).await)),
            "set_post_channel" => {
                host.post_channel = step["channel_id"].as_str().map(str::to_owned);
                Ok(step["channel_id"].clone())
            }
            "set_config" => {
                let value = text(&step["value"]).to_owned();
                host.config.insert(config_key(&step["key"]), value.clone());
                Ok(json!(value))
            }
            "set_weekly_digest" => {
                let week = utc(&step["week_start"]);
                host.set_digest(
                    week,
                    text(&step["channel_id"]),
                    text(&step["message_id"]),
                    now,
                );
                Ok(Value::Null)
            }
            "retire_weekly_digests_before" => {
                Ok(json!(host.retire_before(utc(&step["week_start"]), now)))
            }
            "set_clock" => {
                let at = instant(&step["clock"]);
                clock.set(at);
                Ok(json!(isoformat(&at)))
            }
            "set_transport" => {
                host.journal.fail_sends = step["fail_sends"].as_bool().expect("fail_sends");
                Ok(json!(host.journal.fail_sends))
            }
            other => panic!("unknown digest vector operation {other:?}"),
        };
        steps.push(step_result(step, result));
    }
    let mut state = common::final_state(&common::snapshot(&service).await, true);
    state["side_effects"] = json!(host.journal.effects);
    let extra = host.state();
    state["weekly_digests"] = extra["weekly_digests"].clone();
    state["config"] = extra["config"].clone();
    json!({ "steps": steps, "final_state": state })
}

#[tokio::test]
async fn every_digest_case_replays_exactly() {
    let file = common::load("digest.json");
    assert_eq!(file["family"], "digest");
    assert_eq!(file["schema_version"], "v5-scheduler-digest-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let mut replayed = 0;
    for case in cases {
        let case_id = text(&case["case_id"]);
        let actual = replay(case).await;
        let expected = &case["expected"];
        assert_case(case_id, &actual, expected);
        for table in ["weekly_digests", "config"] {
            assert_eq!(
                actual["final_state"][table], expected["final_state"][table],
                "{case_id}: final {table}"
            );
        }
        replayed += 1;
    }
    assert_eq!(replayed, cases.len(), "skipped cases");
}
