//! Replays `mutations.json`: status, amend, swap and weekly-timing edits with
//! the notices they ask for, through the scheduler service.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{NaiveDate, NaiveTime, Utc};
use kanade::domain::{
    catalog::{BossSpec, BossTable, CatalogSpec, DifficultySpec},
    members::{Directory, Member, PingLevel},
    notify::{ChannelDirectory, DeliverySettings, plan_notice},
    schedule::{
        FixedEdit, FixedRun, NewFixedRun, NewRun, Notice, ReminderPolicy, RsvpSource, RsvpState,
        RunSource, RunState, RunStatus, SchedulePolicy, StatusChange, settable_status,
    },
    time::ZonedDateTime,
    weeks,
};
use serde_json::{Value, json};

use crate::common::{
    self, Deviation, Service, apply_deviations, assert_case, clock_time, countdowns, instant,
    register_materialised, run_ref, step_result, strings, text, utc, weekday, zone,
};

/// Decision (a): a weekly participants edit drops the answers of members it
/// took off the upcoming runs (and re-derives status; no frozen status
/// differs). v4 kept 1002's `yes` on run 002 after the edit replaced 1002 with
/// 1003.
fn fixed_edit_drops_removed_rsvps() -> Vec<Deviation> {
    vec![Deviation {
        case_id: "update-fixed-pushes-only-touched-fields-to-live-runs",
        pointer: "/final_state/rsvps".into(),
        v4: json!([{
            "at": "2026-08-26T17:00:00+00:00",
            "run_id": "00000000-0000-4000-8002-000000000002",
            "source": "chat",
            "state": "yes",
            "user_id": "1002",
        }]),
        v5: json!([]),
    }]
}

/// Steps whose outcome is decided before the scheduler: v4 `parse_when`
/// (dateparser free text) is a surface-adapter concern outside this slice. The
/// replay checks the step is a refusal that changed nothing and moves on.
const TEXT_PARSING_OWNED_ELSEWHERE: &[(&str, usize)] =
    &[("amend-resets-answers-status-and-refuses-occupied-week", 7)];

/// v4 `_audit` draws one id for every mutation it records. The audit log is
/// a later slice, so the replayer draws that id in its place after each
/// recorded mutation, keeping every later id aligned.
fn draw_audit_id(ids: &mut common::SeqIds) {
    use kanade::domain::ids::IdGenerator;
    ids.new_id();
}

struct Guild {
    members: BTreeMap<String, Member>,
    watched: BTreeSet<u64>,
}

impl Guild {
    fn new(input: &Value) -> Self {
        let members = input["members"]
            .as_array()
            .expect("members")
            .iter()
            .map(|member| {
                let user_id = text(&member["user_id"]).to_owned();
                let entry = Member {
                    user_id: user_id.clone(),
                    display_name: member["display_name"].as_str().map(Into::into),
                    nickname: member["nickname"].as_str().map(Into::into),
                    has_role: member["has_role"].as_bool().expect("has_role"),
                    is_bot: false,
                    ping_level: PingLevel::parse_stored(text(&member["ping_level"]))
                        .expect("ping level"),
                };
                (user_id, entry)
            })
            .collect();
        let watched = strings(&input["watched_channel_ids"])
            .iter()
            .map(|id| id.parse().expect("numeric channel id"))
            .collect();
        Self { members, watched }
    }
}

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        self.members.get(user_id).cloned()
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        channel_id
            .parse::<u64>()
            .is_ok_and(|id| self.watched.contains(&id))
    }
}

/// The v4 host stub resolves any channel id; the vectors set no post channel.
struct AnyChannel;

impl ChannelDirectory for AnyChannel {
    fn is_reachable(&self, _channel_id: &str) -> bool {
        true
    }
}

fn catalog(raw: &Value) -> BossTable {
    let difficulties = raw["difficulties"]
        .as_array()
        .expect("difficulties")
        .iter()
        .map(|entry| DifficultySpec {
            prefix: text(&entry["prefix"]).into(),
            label: text(&entry["label"]).into(),
        })
        .collect();
    let bosses = raw["bosses"]
        .as_array()
        .expect("bosses")
        .iter()
        .map(|entry| BossSpec {
            short: text(&entry["short"]).into(),
            full: entry["full"].as_str().map(Into::into),
            level: entry["level"].as_i64(),
            difficulties: entry.get("difficulties").map(strings),
            aliases: entry.get("aliases").map(strings).unwrap_or_default(),
            ..BossSpec::default()
        })
        .collect();
    BossTable::from_spec(&CatalogSpec {
        difficulties,
        bosses,
    })
    .expect("synthetic catalog is valid")
}

fn run_view(state: &RunState) -> Value {
    let run = &state.run;
    json!({
        "id": run.id,
        "status": run.status.as_str(),
        "datetime": common::iso(run.datetime),
        "week_start": common::iso(run.week_start),
        "channel_id": run.channel_id,
        "bosses": run.bosses,
        "participants": run.participants.iter().map(|uid| json!({
            "id": uid,
            "rsvp": state.rsvps.get(uid).map(|state| state.as_str()),
        })).collect::<Vec<_>>(),
        "roster_change": {
            "out": state.roster_change.out,
            "in": state.roster_change.joined,
            "changed": state.roster_change.changed(),
        },
    })
}

fn fixed_view(fixed: &FixedRun, guild: &Guild) -> Value {
    json!({
        "id": fixed.id,
        "bosses": fixed.bosses,
        "weekday": fixed.weekday.num_days_from_monday(),
        "time": common::hhmm(fixed.time),
        "participants": fixed.participants,
        "channel_id": fixed.channel_id,
        "channel_watched": fixed.channel_id.as_deref().is_some_and(|id| guild.is_watched(id)),
        "note": fixed.note,
    })
}

/// The harness's strict reader for the `YYYY-MM-DD HH:MM` guild-local form.
fn read_when(text: &str, policy: &SchedulePolicy) -> Option<chrono::DateTime<Utc>> {
    let (date, time) = text.split_once(' ')?;
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let (hour, minute) = time.split_once(':')?;
    let time = NaiveTime::from_hms_opt(hour.parse().ok()?, minute.parse().ok()?, 0)?;
    let zoned = ZonedDateTime::new(date.and_time(time), policy.zone()).ok()?;
    Some(zoned.to_fixed().with_timezone(&Utc))
}

fn edit_from(changes: &Value, table: &BossTable) -> Result<FixedEdit, String> {
    let changes = changes.as_object().expect("changes");
    for key in changes.keys() {
        assert!(
            [
                "bosses",
                "day",
                "time",
                "participants",
                "channel_id",
                "note"
            ]
            .contains(&key.as_str()),
            "unexpected fixed change {key}"
        );
    }
    let field = |key: &str| changes.get(key).map(text);
    Ok(FixedEdit {
        bosses: field("bosses")
            .map(|raw| table.parse(raw))
            .transpose()
            .map_err(|error| error.to_string())?,
        weekday: field("day")
            .map(weeks::parse_weekday)
            .transpose()
            .map_err(|error| error.to_string())?,
        time: field("time")
            .map(weeks::parse_hhmm)
            .transpose()
            .map_err(|error| error.to_string())?,
        participants: changes.get("participants").map(strings),
        channel_id: field("channel_id").map(Into::into),
        note: field("note").map(Into::into),
        owner_id: None,
    })
}

struct Replay<'a> {
    case_id: &'a str,
    service: Service,
    clock: common::TestClock,
    ids: common::SeqIds,
    guild: Guild,
    table: BossTable,
    policy: SchedulePolicy,
    fixed_refs: BTreeMap<String, String>,
    run_refs: BTreeMap<String, String>,
    effects: Vec<Value>,
    skipped: usize,
}

impl Replay<'_> {
    fn record(&mut self, notices: &[Notice]) {
        let settings = DeliverySettings {
            post_channel_id: None,
            quiet_mode: false,
            attendance: kanade::domain::attendance::AttendancePolicy::V4_COMPAT,
        };
        for notice in notices {
            // Dropped when there is no channel to post in, as v4 `_announce`.
            let Some(intent) = plan_notice(notice, &self.guild, &AnyChannel, settings) else {
                continue;
            };
            self.effects.push(json!({
                "channel_id": intent.channel_id,
                "mentions": intent.mentions,
                "effect_kind": intent.effect.as_str(),
                "effect_context": intent.effect_context,
            }));
        }
    }

    async fn register(&mut self) {
        let now = self.clock_now();
        for week in self.policy.materialised_weeks(now).expect("weeks") {
            let instant = week.to_fixed().with_timezone(&Utc);
            register_materialised(
                &self.service,
                &self.fixed_refs,
                instant,
                &week.isoformat(),
                &mut self.run_refs,
            )
            .await;
        }
    }

    async fn status_of(&self, run: &str) -> RunStatus {
        let state = common::snapshot(&self.service).await;
        state
            .runs
            .iter()
            .find(|row| row.id == run)
            .expect("known run")
            .status
    }

    fn clock_now(&self) -> chrono::DateTime<Utc> {
        use kanade::domain::scheduler::Clock;
        self.clock.now()
    }

    async fn step(&mut self, index: usize, step: &Value, expected: &Value) -> Value {
        let result: Result<Value, String> = match text(&step["op"]) {
            "add_fixed" => {
                let id = self
                    .service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .add_fixed_run(NewFixedRun {
                        owner_pinned: false,
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
                self.fixed_refs
                    .insert(text(&step["fixed_key"]).to_owned(), id.clone());
                Ok(json!(id))
            }
            "create_run" => {
                let id = self
                    .service
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
                self.run_refs
                    .insert(text(&step["run_key"]).to_owned(), id.clone());
                Ok(json!(id))
            }
            "set_rsvp" => {
                let run = run_ref(&self.run_refs, &step["run_key"]).to_owned();
                let state = RsvpState::parse(text(&step["state"])).unwrap();
                let source = RsvpSource::parse(text(&step["source"])).unwrap();
                self.service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .set_rsvp(&run, text(&step["user_id"]), state, source)
                    .await
                    .map(|()| json!(state.as_str()))
                    .map_err(|error| error.to_string())
            }
            "set_clock" => {
                let at = instant(&step["clock"]);
                self.clock.set(at);
                Ok(json!(kanade::domain::time::isoformat(&at)))
            }
            "materialise_weeks" => {
                let result = self
                    .service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .materialise_weeks(&self.policy)
                    .await;
                self.register().await;
                result
                    .map(|_| Value::Null)
                    .map_err(|error| error.to_string())
            }
            "set_status" => match settable_status(text(&step["status"])) {
                Err(error) => Err(error.to_string()),
                Ok(status) => {
                    let run = run_ref(&self.run_refs, &step["run_key"]).to_owned();
                    let change = StatusChange {
                        status,
                        announce: step["announce"].as_bool().expect("announce"),
                        via_portal: step["mark"].as_bool().expect("mark"),
                    };
                    let before = self.status_of(&run).await;
                    match self
                        .service
                        .as_origin(kanade::domain::history::Origin::for_tests())
                        .set_status(&run, change, &self.policy.reminders)
                        .await
                    {
                        Ok(outcome) => {
                            if before != status {
                                draw_audit_id(&mut self.ids);
                            }
                            self.record(&outcome.notices);
                            Ok(run_view(&outcome.value))
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
            },
            "amend" => {
                let run = run_ref(&self.run_refs, &step["run_key"]).to_owned();
                match read_when(text(&step["to"]), &self.policy) {
                    None => {
                        assert!(
                            TEXT_PARSING_OWNED_ELSEWHERE.contains(&(self.case_id, index)),
                            "{} step {index}: unreadable amend target",
                            self.case_id
                        );
                        assert!(expected.get("error").is_some(), "must be a refusal");
                        self.skipped += 1;
                        return expected.clone();
                    }
                    Some(to) => match self
                        .service
                        .as_origin(kanade::domain::history::Origin::for_tests())
                        .amend_run(&run, to, &self.policy)
                        .await
                    {
                        Ok(outcome) => {
                            draw_audit_id(&mut self.ids);
                            self.record(&outcome.notices);
                            Ok(run_view(&outcome.value))
                        }
                        Err(error) => Err(error.to_string()),
                    },
                }
            }
            "swap" => {
                let run = run_ref(&self.run_refs, &step["run_key"]).to_owned();
                let remove = strings(&step["remove"]);
                let add = strings(&step["add"]);
                let via_portal = step["mark"].as_bool().expect("mark");
                match self
                    .service
                    .as_origin(kanade::domain::history::Origin::for_tests())
                    .swap_participants(&run, &remove, &add, via_portal, &self.guild)
                    .await
                {
                    Ok(outcome) => {
                        if !outcome.notices.is_empty() {
                            draw_audit_id(&mut self.ids);
                        }
                        self.record(&outcome.notices);
                        Ok(run_view(&outcome.value))
                    }
                    Err(error) => Err(error.to_string()),
                }
            }
            "update_fixed" => {
                let fixed = self.fixed_refs[text(&step["fixed_key"])].clone();
                let result = match edit_from(&step["changes"], &self.table) {
                    Err(error) => Err(error),
                    Ok(edit) => match self
                        .service
                        .as_origin(kanade::domain::history::Origin::for_tests())
                        .update_fixed(&fixed, edit, &self.guild, &self.policy)
                        .await
                    {
                        Ok(outcome) => {
                            draw_audit_id(&mut self.ids);
                            self.record(&outcome.notices);
                            Ok(fixed_view(&outcome.value, &self.guild))
                        }
                        Err(error) => Err(error.to_string()),
                    },
                };
                self.register().await;
                result
            }
            other => panic!("unknown mutations vector operation {other:?}"),
        };
        step_result(step, result)
    }
}

async fn replay(case: &Value) -> (Value, usize) {
    let input = &case["input"];
    let (service, clock, ids) = common::service_with_ids(input);
    let policy = SchedulePolicy::new(
        ReminderPolicy {
            zone: zone(input),
            ping_time: clock_time(&input["ping_time"]),
            countdowns: countdowns(&input["countdowns"]),
        },
        weekday(&input["reset_weekday"]),
        clock_time(&input["reset_time"]),
    );
    let mut replay = Replay {
        case_id: text(&case["case_id"]),
        service,
        clock,
        ids,
        guild: Guild::new(input),
        table: catalog(&input["catalog"]),
        policy,
        fixed_refs: BTreeMap::new(),
        run_refs: BTreeMap::new(),
        effects: Vec::new(),
        skipped: 0,
    };
    let expected_steps = case["expected"]["steps"].as_array().expect("steps");
    let mut steps = Vec::new();
    for (index, step) in input["steps"].as_array().expect("steps").iter().enumerate() {
        steps.push(replay.step(index, step, &expected_steps[index]).await);
        // No frozen case moves the clock, so no tick backlog can exist and every
        // run must be free of already-due pings after each step.
        assert_ne!(text(&step["op"]), "set_clock", "revisit the due-ping scope");
        let state = common::snapshot(&replay.service).await;
        common::assert_sound(&state, replay.clock_now(), None);
    }
    let mut final_state = common::final_state(&common::snapshot(&replay.service).await, true);
    final_state["side_effects"] = json!(replay.effects);
    (
        json!({ "steps": steps, "final_state": final_state }),
        replay.skipped,
    )
}

#[tokio::test]
async fn every_mutation_case_replays_exactly_except_named_deviations() {
    let file = common::load("mutations.json");
    assert_eq!(file["family"], "mutations");
    assert_eq!(file["schema_version"], "v5-scheduler-mutations-v1");
    let cases = file["cases"].as_array().expect("cases");
    assert!(!cases.is_empty());
    let deviations = fixed_edit_drops_removed_rsvps();
    let (mut replayed, mut used, mut skipped) = (0, 0, 0);
    for case in cases {
        let case_id = text(&case["case_id"]);
        let (expected, applied) = apply_deviations(case_id, &case["expected"], &deviations);
        let (actual, skips) = replay(case).await;
        assert_case(case_id, &actual, &expected);
        replayed += 1;
        used += applied;
        skipped += skips;
    }
    assert_eq!(replayed, cases.len(), "skipped cases");
    assert_eq!(used, deviations.len(), "unused deviations");
    assert_eq!(
        skipped,
        TEXT_PARSING_OWNED_ELSEWHERE.len(),
        "unused parse exemptions"
    );
}
