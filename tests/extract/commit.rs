//! Replays `commit.json` (v4 `bot.extract.commit`) through the v5 proposal
//! path: propose → approve → the shared draft merge, on the in-memory store
//! with the vector's pinned clock and uuid sequence.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use kanade::domain::drafts::{DraftStatus, ProposalSource, ProposalStore};
use kanade::domain::history::Origin;
use kanade::domain::ids::IdGenerator;
use kanade::domain::members::{Directory, Member};
use kanade::domain::proposals::{
    Approver, ChangeKind, Payload, ProposedChange, Refusal, may_commit,
};
use kanade::domain::schedule::{
    NewFixedRun, NewRun, ReminderPolicy, RsvpSource, RsvpState, RunSource, RunStatus,
    SchedulePolicy,
};
use kanade::domain::scheduler::{ProposalError, ProposalRequest, Supersede, SupersedeScope};
use serde_json::{Value, json};

use crate::common::{
    self, Deviation, SeqIds, TestClock, apply_deviations, clock_time, countdowns, iso, opt_iso,
    strings, text, utc, weekday, zone,
};

/// The vector's roster: every member holds the bossing role.
struct Guild(Vec<String>);

impl Directory for Guild {
    fn member(&self, user_id: &str) -> Option<Member> {
        self.0.iter().any(|id| id == user_id).then(|| Member {
            user_id: user_id.to_owned(),
            has_role: true,
            ..Member::default()
        })
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

/// One proposed amendment as v4's `amendments` row tracks it.
struct Row {
    id: String,
    kind: ChangeKind,
    run_id: Option<String>,
    channel_id: Option<String>,
    new_datetime: Option<DateTime<Utc>>,
    participants: Vec<String>,
    payload: Value,
    /// `D-PROPOSE-REFUSES`: the refusal instead of a proposal.
    refused: Option<String>,
}

struct Replay {
    service: common::Service,
    clock: TestClock,
    ids: SeqIds,
    policy: SchedulePolicy,
    guild: Guild,
    runs: BTreeMap<String, String>,
    fixed: BTreeMap<String, String>,
    rows: BTreeMap<String, Row>,
    order: Vec<String>,
}

fn opt(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn admin() -> Approver {
    Approver {
        user_id: "admin".into(),
        has_role: true,
        is_admin: true,
        via_portal: false,
    }
}

impl Replay {
    fn new(input: &Value) -> Self {
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
        Self {
            service,
            clock,
            ids,
            policy,
            guild: Guild(strings(&input["members"])),
            runs: BTreeMap::new(),
            fixed: BTreeMap::new(),
            rows: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    fn run(&self, key: &Value) -> Option<String> {
        key.as_str().map(|key| self.runs[key].clone())
    }

    fn row(&self, step: &Value) -> &Row {
        &self.rows[text(&step["amendment_key"])]
    }

    fn change(&self, step: &Value) -> (ProposedChange, Value) {
        let kind = ChangeKind::parse(text(&step["kind"])).expect("kind");
        let raw = &step["payload"];
        let list = |name: &str| raw.get(name).map(strings).unwrap_or_default();
        let fixed = opt(&step["fixed_key"]).map(|key| self.fixed[&key].clone());
        let slot = || {
            (
                raw.get("weekday").map(weekday),
                raw.get("time").map(clock_time),
            )
        };
        let payload = match kind {
            ChangeKind::Sub => Payload::Sub {
                remove: list("remove"),
                add: list("add"),
            },
            ChangeKind::Split => Payload::Split {
                bosses: raw.get("bosses").map(strings),
                participants: list("participants"),
            },
            ChangeKind::Fix => match raw.get("op").and_then(Value::as_str) {
                Some("edit") => {
                    let (weekday, time) = slot();
                    Payload::FixEdit {
                        fixed_run_id: fixed.clone(),
                        weekday,
                        time,
                        participants: list("participants"),
                    }
                }
                Some("remove") => Payload::FixRemove {
                    fixed_run_id: fixed.clone(),
                },
                _ => {
                    let (weekday, time) = slot();
                    Payload::Fix { weekday, time }
                }
            },
            _ => Payload::None,
        };
        // v4 stores the timing an edit or removal names in the payload.
        let mut stored = raw.clone();
        if let Some(fixed) = fixed {
            stored["fixed_run_id"] = json!(fixed);
        }
        let change = ProposedChange {
            kind,
            run_id: self.run(&step["run_key"]),
            channel_id: opt(&step["channel_id"]),
            bosses: strings(&step["bosses"]),
            participants: strings(&step["participants"]),
            new_datetime: (!step["new_datetime"].is_null()).then(|| utc(&step["new_datetime"])),
            rsvp: opt(&step["rsvp"]).map(|state| RsvpState::parse(&state).expect("rsvp")),
            payload,
        };
        (change, stored)
    }

    async fn step(&mut self, step: &Value) -> Value {
        match text(&step["op"]) {
            "create_run" => {
                let id = self
                    .service
                    .as_origin(Origin::for_tests())
                    .create_run(NewRun {
                        fixed_run_id: None,
                        channel_id: opt(&step["channel_id"]),
                        week_start: utc(&step["week_start"]),
                        datetime: utc(&step["at"]),
                        bosses: strings(&step["bosses"]),
                        participants: strings(&step["participants"]),
                        status: RunStatus::parse(text(&step["status"])).expect("status"),
                        source: RunSource::Amend,
                    })
                    .await
                    .expect("create_run");
                self.runs.insert(text(&step["run_key"]).into(), id.clone());
                json!(id)
            }
            "add_fixed" => {
                let id = self
                    .service
                    .as_origin(Origin::for_tests())
                    .add_fixed_run(NewFixedRun {
                        owner_pinned: false,
                        owner_id: text(&step["owner_id"]).into(),
                        channel_id: opt(&step["channel_id"]),
                        bosses: strings(&step["bosses"]),
                        weekday: weekday(&step["weekday"]),
                        time: clock_time(&step["time"]),
                        participants: strings(&step["participants"]),
                        note: None,
                    })
                    .await
                    .expect("add_fixed");
                self.fixed
                    .insert(text(&step["fixed_key"]).into(), id.clone());
                json!(id)
            }
            "rematerialise" => {
                self.service
                    .as_origin(Origin::for_tests())
                    .materialise_weeks(&self.policy)
                    .await
                    .expect("materialise");
                Value::Null
            }
            "find_fixed_run" => {
                let fixed = &self.fixed[text(&step["fixed_key"])];
                let week = utc(&step["week_start"]);
                let state = common::snapshot(&self.service).await;
                let run = state
                    .runs
                    .iter()
                    .find(|run| run.fixed_run_id.as_ref() == Some(fixed) && run.week_start == week)
                    .expect("materialised run");
                self.runs
                    .insert(text(&step["run_key"]).into(), run.id.clone());
                json!(run.id)
            }
            "set_rsvp" => {
                let run = self.run(&step["run_key"]).expect("run");
                let state = RsvpState::parse(text(&step["state"])).expect("state");
                self.service
                    .as_origin(Origin::for_tests())
                    .set_rsvp(&run, text(&step["user_id"]), state, RsvpSource::Reaction)
                    .await
                    .expect("set_rsvp");
                json!(state.as_str())
            }
            "set_clock" => {
                self.clock.set(common::instant(&step["clock"]));
                step["clock"].clone()
            }
            "propose" => self.propose(step).await,
            "may_commit" => {
                let row = self.row(step);
                let state = common::snapshot(&self.service).await;
                let run = self
                    .run(&step["run_key"])
                    .and_then(|id| state.runs.iter().find(|run| run.id == id).cloned());
                json!(may_commit(
                    &row.participants,
                    run.as_ref().map(|run| run.participants.as_slice()),
                    text(&step["user_id"]),
                    common_flag(&step["has_role"]),
                    common_flag(&step["is_admin"]),
                    common_flag(&step["is_owner"]),
                ))
            }
            "commit" => self.commit(step).await,
            "supersede" => {
                let run = self.run(&step["run_key"]);
                let keep = opt(&step["keep_key"]).map(|key| self.rows[&key].id.clone());
                let channel = opt(&step["channel_id"]);
                let from = opt(&step["from_channel"]);
                let bosses = strings(&step["bosses"]);
                let ids = self
                    .service
                    .supersede_proposals(SupersedeScope {
                        run_id: run.as_deref(),
                        channel_id: channel.as_deref(),
                        bosses: &bosses,
                        keep: keep.as_deref(),
                        from_channel: from.as_deref(),
                        by: ProposalSource::Extraction,
                    })
                    .await
                    .expect("supersede");
                json!(ids)
            }
            "reject" => {
                let id = self.row(step).id.clone();
                self.service
                    .reject_proposal(&id, &admin())
                    .await
                    .expect("reject");
                json!("rejected")
            }
            "expire_stale" => json!(self.service.expire_due_proposals().await.expect("expire")),
            other => panic!("unknown commit vector operation {other:?}"),
        }
    }

    async fn propose(&mut self, step: &Value) -> Value {
        let (change, payload) = self.change(step);
        let key = text(&step["amendment_key"]).to_owned();
        let proposed = self
            .service
            .propose(
                ProposalRequest {
                    change: change.clone(),
                    source: ProposalSource::Extraction,
                    source_id: key.clone(),
                    // v4's `propose` supersedes nothing; the vector does it
                    // with explicit `supersede` steps and on commit.
                    supersede: Supersede::Keep,
                },
                &self.policy,
                &self.guild,
            )
            .await;
        let (id, refused, value) = match proposed {
            Ok(proposed) => {
                let id = proposed.proposal.id;
                (id.clone(), None, json!(id))
            }
            // D-PROPOSE-REFUSES: nothing is written; the v4 id is burned so
            // later ids stay aligned.
            Err(ProposalError::Refused(refusal)) => (
                self.ids.clone().new_id(),
                Some(refusal.to_string()),
                json!({ "refused": refusal.to_string() }),
            ),
            Err(error) => panic!("{key}: propose failed: {error}"),
        };
        self.order.push(key.clone());
        self.rows.insert(
            key,
            Row {
                id,
                kind: change.kind,
                run_id: change.run_id,
                channel_id: change.channel_id,
                new_datetime: change.new_datetime,
                participants: change.participants,
                payload,
                refused,
            },
        );
        value
    }

    async fn commit(&mut self, step: &Value) -> Value {
        let key = text(&step["amendment_key"]).to_owned();
        let row = &self.rows[&key];
        // The card's own channel is where ✅ lands; the vector passes it or null.
        if let Some(channel) = opt(&step["channel_id"]) {
            assert_eq!(
                Some(&channel),
                row.channel_id.as_ref(),
                "{key}: commit channel"
            );
        }
        if let Some(refused) = &row.refused {
            return json!({ "refused": refused });
        }
        let id = row.id.clone();
        let approver = Approver {
            user_id: text(&step["actor_id"]).into(),
            has_role: true,
            is_admin: true,
            via_portal: false,
        };
        let approved = self
            .service
            .approve_proposal(&id, &approver, &self.policy, &self.guild)
            .await;
        let approved = match approved {
            Ok(approved) => approved,
            Err(error) => return json!({ "error": error.to_string() }),
        };
        assert!(
            approved.follow_up_errors.is_empty(),
            "{key}: follow-up failed"
        );
        let row = self.rows.get_mut(&key).expect("row");
        if row.kind == ChangeKind::Add {
            row.run_id.clone_from(&approved.run_id);
        }
        let removing = row.payload.get("op").and_then(Value::as_str) == Some("remove");
        let callbacks = match &approved.fixed_run_id {
            Some(fixed) if step["rematerialise"] == true && !removing => vec![fixed.clone()],
            _ => Vec::new(),
        };
        json!({
            "amendment_id": id,
            "kind": approved.kind.as_str(),
            "applied": true,
            "run_id": approved.run_id,
            "fixed_run_id": approved.fixed_run_id,
            "created_run_ids": approved.created_run_ids,
            "old_datetime": opt_iso(approved.old_datetime),
            "problem": null,
            "superseded": approved.superseded,
            "notes": approved.notes,
            "fixed_callbacks": callbacks,
        })
    }

    async fn final_state(&self) -> Value {
        let mut state = common::final_state(&common::snapshot(&self.service).await, true);
        let mut amendments = Vec::new();
        for key in &self.order {
            let row = &self.rows[key];
            let status = match &row.refused {
                Some(_) => "refused",
                None => {
                    let (loaded, _) = self
                        .service
                        .store()
                        .load_proposal(&row.id)
                        .await
                        .expect("load")
                        .expect("proposal");
                    match loaded.draft.status {
                        DraftStatus::Submitted => "proposed",
                        DraftStatus::Merged => "confirmed",
                        DraftStatus::Rejected => "rejected",
                        DraftStatus::Expired => "expired",
                        DraftStatus::Discarded
                            if loaded.draft.close_reason.as_deref() == Some("superseded") =>
                        {
                            "superseded"
                        }
                        other => panic!("{key}: unexpected status {other}"),
                    }
                }
            };
            amendments.push(json!({
                "id": row.id,
                "kind": row.kind.as_str(),
                "status": status,
                "run_id": row.run_id,
                "new_datetime": row.new_datetime.map(iso),
                "payload": row.payload,
            }));
        }
        state["amendments"] = json!(amendments);
        state.as_object_mut().expect("state").remove("side_effects");
        state
    }
}

fn common_flag(value: &Value) -> bool {
    value.as_bool().expect("boolean")
}

/// Named v5 differences; every entry must meet its frozen v4 value.
struct Named {
    name: &'static str,
    entries: Vec<Deviation>,
}

fn dev(case_id: &'static str, pointer: &str, v4: Value, v5: Value) -> Deviation {
    Deviation {
        case_id,
        pointer: pointer.to_owned(),
        v4,
        v5,
    }
}

fn uid(n: u32) -> String {
    format!("00000000-0000-4000-8003-{n:012}")
}

/// One `D-PROPOSE-REFUSES` amendment: v4 carded it (id `n`) and refused it
/// at ✅ (`commit` step) with the same words v5 now refuses it with up front.
struct Refused {
    case_id: &'static str,
    propose: usize,
    commit: Option<(usize, Option<u32>, Option<&'static str>)>,
    n: u32,
    kind: &'static str,
    refusal: Refusal,
    amendment: usize,
    v4_status: &'static str,
}

fn refused(list: &[Refused]) -> Vec<Deviation> {
    let mut entries = Vec::new();
    for item in list {
        let text = item.refusal.to_string();
        let v5 = json!({ "refused": text });
        entries.push(dev(
            item.case_id,
            &format!("/steps/{}/value", item.propose),
            json!(uid(item.n)),
            v5.clone(),
        ));
        if let Some((step, run, old)) = item.commit {
            let v4 = json!({
                "amendment_id": uid(item.n),
                "applied": false,
                "created_run_ids": [],
                "fixed_callbacks": [],
                "fixed_run_id": null,
                "kind": item.kind,
                "notes": [],
                "old_datetime": old,
                "problem": text,
                "run_id": run.map(uid),
                "superseded": [],
            });
            entries.push(dev(item.case_id, &format!("/steps/{step}/value"), v4, v5));
        }
        entries.push(dev(
            item.case_id,
            &format!("/final_state/amendments/{}/status", item.amendment),
            json!(item.v4_status),
            json!("refused"),
        ));
    }
    entries
}

fn deviations() -> Vec<Named> {
    use Refusal::*;
    let r = |case_id, propose, commit, n, kind, refusal, amendment, v4_status| Refused {
        case_id,
        propose,
        commit,
        n,
        kind,
        refusal,
        amendment,
        v4_status,
    };
    let (who, mv, week, add, cancel, subs, split, rsvp, fix, ttl) = (
        "who-may-confirm",
        "move-reschedules-and-refuses",
        "move-into-a-week-its-weekly-already-holds",
        "add-creates-a-run-or-refuses",
        "cancel-and-own-time",
        "stand-ins",
        "split-a-run",
        "carded-answers",
        "weekly-timings-from-chat",
        "supersede-reject-and-expire",
    );
    let moved = Some("2026-08-31T13:30:00+00:00");
    let mut propose_refuses = refused(&[
        r(who, 7, None, 3, "add", NoDayAndTime, 1, "proposed"),
        r(who, 10, None, 4, "add", NoDayAndTime, 2, "proposed"),
        r(who, 13, None, 5, "fix", NoRecurringSlot, 3, "proposed"),
        r(
            mv,
            4,
            Some((5, None, None)),
            2,
            "move",
            NoNewTime,
            0,
            "superseded",
        ),
        r(
            mv,
            9,
            Some((10, None, None)),
            8,
            "move",
            RunGone,
            3,
            "proposed",
        ),
        r(
            week,
            3,
            Some((4, Some(2), moved)),
            14,
            "move",
            WeeklyHoldsWeek,
            0,
            "superseded",
        ),
        r(
            add,
            0,
            Some((1, None, None)),
            1,
            "add",
            NoDayAndTime,
            0,
            "proposed",
        ),
        r(
            add,
            2,
            Some((3, None, None)),
            2,
            "add",
            NoBosses,
            1,
            "proposed",
        ),
        r(add, 5, None, 4, "add", NoDayAndTime, 3, "superseded"),
        r(add, 6, None, 5, "add", NoDayAndTime, 4, "proposed"),
        r(
            cancel,
            6,
            Some((7, None, None)),
            6,
            "cancel",
            RunGone,
            2,
            "proposed",
        ),
        r(
            subs,
            6,
            Some((7, Some(1), None)),
            3,
            "sub",
            NobodyToSwap,
            1,
            "proposed",
        ),
        r(
            subs,
            9,
            Some((10, Some(4), None)),
            5,
            "sub",
            RunEmptied,
            2,
            "superseded",
        ),
        r(
            split,
            3,
            Some((4, None, None)),
            10,
            "split",
            NoBossesFromRun,
            1,
            "proposed",
        ),
        r(
            rsvp,
            7,
            Some((8, None, None)),
            4,
            "rsvp",
            AnswerForOutsider,
            2,
            "proposed",
        ),
        r(
            rsvp,
            9,
            Some((10, None, None)),
            5,
            "rsvp",
            NoAnswer,
            3,
            "proposed",
        ),
        r(
            rsvp,
            11,
            Some((12, None, None)),
            6,
            "rsvp",
            NobodyNamed,
            4,
            "proposed",
        ),
        r(
            fix,
            1,
            Some((2, None, None)),
            2,
            "fix",
            NoRecurringSlot,
            0,
            "superseded",
        ),
        r(
            fix,
            9,
            Some((10, None, None)),
            69,
            "fix",
            NothingLeftToChange,
            3,
            "proposed",
        ),
        r(
            fix,
            13,
            Some((14, None, None)),
            71,
            "fix",
            TimingAlreadyGone,
            5,
            "proposed",
        ),
        r(ttl, 6, None, 5, "add", NoDayAndTime, 3, "superseded"),
        r(ttl, 7, None, 6, "add", NoDayAndTime, 4, "expired"),
    ]);
    // What the refused amendments were no longer there to be retired by.
    for (case_id, pointer, v4, v5) in [
        (
            mv,
            "/steps/8/value/superseded",
            json!([uid(2), uid(4)]),
            json!([uid(4)]),
        ),
        (
            week,
            "/steps/6/value/superseded",
            json!([uid(14)]),
            json!([]),
        ),
        (add, "/steps/7/value/superseded", json!([uid(4)]), json!([])),
        (
            subs,
            "/steps/12/value/superseded",
            json!([uid(5)]),
            json!([]),
        ),
        (fix, "/steps/4/value/superseded", json!([uid(2)]), json!([])),
        (ttl, "/steps/8/value", json!([uid(5)]), json!([])),
    ] {
        propose_refuses.push(dev(case_id, pointer, v4, v5));
    }
    // v5 has no `manual` run source; the fixture's hand-made runs are `amend`.
    let manual = [
        (who, 0),
        (mv, 0),
        (cancel, 0),
        (cancel, 1),
        (subs, 0),
        (subs, 1),
        (split, 0),
        (split, 2),
        (split, 4),
        (rsvp, 0),
        (fix, 1),
        (ttl, 0),
    ]
    .into_iter()
    .map(|(case_id, run)| {
        dev(
            case_id,
            &format!("/final_state/runs/{run}/source"),
            json!("manual"),
            json!("amend"),
        )
    })
    .collect();
    vec![
        Named {
            name: "D-PROPOSE-REFUSES",
            entries: propose_refuses,
        },
        Named {
            name: "D-NO-MANUAL-SOURCE",
            entries: manual,
        },
        Named {
            // The store expires at `expires_at <= now`; v4 only strictly
            // after 24 h. The second pass finds `late` already expired (and
            // `add2` was never proposed, D-PROPOSE-REFUSES).
            name: "D-TTL-BOUNDARY",
            entries: vec![
                dev(ttl, "/steps/13/value", json!([]), json!([uid(7)])),
                dev(ttl, "/steps/15/value", json!([uid(6), uid(7)]), json!([])),
            ],
        },
        Named {
            // v4 applied a proposal `expire_stale` had just expired.
            name: "D-EXPIRED-REFUSED",
            entries: vec![
                dev(
                    ttl,
                    "/steps/16/value",
                    json!({
                        "amendment_id": uid(7),
                        "applied": true,
                        "created_run_ids": [],
                        "fixed_callbacks": [],
                        "fixed_run_id": null,
                        "kind": "cancel",
                        "notes": [],
                        "old_datetime": null,
                        "problem": null,
                        "run_id": uid(1),
                        "superseded": [],
                    }),
                    json!({ "error": ProposalError::Expired.to_string() }),
                ),
                dev(
                    ttl,
                    "/final_state/runs/0/status",
                    json!("cancelled"),
                    json!("planned"),
                ),
                dev(
                    ttl,
                    "/final_state/amendments/5/status",
                    json!("confirmed"),
                    json!("expired"),
                ),
            ],
        },
    ]
}

async fn replay_case(case: &Value) -> Value {
    let input = &case["input"];
    let mut replay = Replay::new(input);
    let mut steps = Vec::new();
    for step in input["steps"].as_array().expect("steps") {
        steps.push(json!({ "value": replay.step(step).await }));
    }
    json!({ "steps": steps, "final_state": replay.final_state().await })
}

#[tokio::test]
async fn the_commit_family_replays_through_propose_approve_and_merge() {
    let file = crate::support::load("commit.json");
    let schema = crate::support::load("commit.schema.json");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    let problems: Vec<String> = validator
        .iter_errors(&file)
        .map(|e| e.to_string())
        .collect();
    assert!(problems.is_empty(), "commit.json: {problems:?}");
    assert_eq!(file["schema_version"], "v5-extract-commit-v1");
    let named = deviations();
    let mut used = vec![0; named.len()];
    let cases = file["cases"].as_array().expect("cases");
    let (mut replayed, mut steps) = (0, 0);
    let mut failures = Vec::new();
    for case in cases {
        let case_id = text(&case["case_id"]);
        let mut expected = case["expected"].clone();
        for (slot, group) in named.iter().enumerate() {
            let (next, applied) = apply_deviations(case_id, &expected, &group.entries);
            expected = next;
            used[slot] += applied;
        }
        let actual = replay_case(case).await;
        let got = actual["steps"].as_array().expect("steps");
        let want = expected["steps"].as_array().expect("steps");
        assert_eq!(got.len(), want.len(), "{case_id}: step count");
        for (index, (got, want)) in got.iter().zip(want).enumerate() {
            if got != want {
                failures.push(format!(
                    "{case_id} step {index}: expected {want}\n  got {got}"
                ));
            }
            steps += 1;
        }
        for table in ["fixed_runs", "runs", "reminders", "rsvps", "amendments"] {
            let (got, want) = (
                &actual["final_state"][table],
                &expected["final_state"][table],
            );
            if got != want {
                failures.push(format!(
                    "{case_id} final {table}: expected {want}\n  got {got}"
                ));
            }
        }
        replayed += 1;
    }
    assert!(
        failures.is_empty(),
        "commit mismatches:\n{}",
        failures.join("\n")
    );
    assert_eq!(replayed, cases.len(), "skipped cases");
    assert_eq!((replayed, steps), (10, 120));
    for (group, used) in named.iter().zip(used) {
        assert_eq!(
            used,
            group.entries.len(),
            "{}: deviation unused",
            group.name
        );
    }
}
