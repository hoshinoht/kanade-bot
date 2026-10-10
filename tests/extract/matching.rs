use kanade::domain::schedule::Run;
use kanade::extract::AmendmentKind;
use kanade::extract::matching::{self, MatchResult};
use serde_json::{Value, json};

use crate::support::{
    Deviation, Outcome, amendment, date, opt_text, replay_family_with, run, select, strings, text,
    unknown_op, zone,
};

fn run_ids(runs: &[&Run]) -> Value {
    json!(runs.iter().map(|run| run.id.as_str()).collect::<Vec<_>>())
}

fn result_json(result: &MatchResult<'_>) -> Value {
    json!({
        "run_id": result.run.map(|run| run.id.as_str()),
        "reason": result.reason,
        "candidate_ids": run_ids(&result.candidates),
        "ambiguous": result.ambiguous,
        "reason_code": result.reason_code,
        "matched": result.matched(),
    })
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let tz = zone(input);
    let pool: Vec<Run> = input["runs"]
        .as_array()
        .expect("runs")
        .iter()
        .map(run)
        .collect();
    let value = match text(&step["op"]) {
        "match_run" => result_json(&matching::match_run(
            &amendment(&step["amendment"]),
            &select(&pool, &step["channel_runs"]),
            &select(&pool, &step["guild_runs"]),
            opt_text(&step["author_id"]),
            &strings(&step["mentioned"]),
        )),
        "runs_spanned" => run_ids(&matching::runs_spanned(
            &amendment(&step["amendment"]),
            &select(&pool, &step["channel_runs"]),
            opt_text(&step["author_id"]),
        )),
        "reachable" => {
            let day = (!step["day"].is_null()).then(|| date(&step["day"]));
            // The frozen operation has no evidence time; pin it to the
            // fixture's first boss week to replay dayless RSVP/Sub reachability
            // under D-EXTRACT-WEEK-ANCHOR.
            run_ids(&matching::reachable(
                &select(&pool, &step["runs"]),
                day,
                tz,
                Some(pool[0].week_start),
            ))
        }
        "needs_run" => {
            let kind = AmendmentKind::parse(text(&step["kind"])).expect("kind");
            json!(matching::needs_run(kind))
        }
        other => unknown_op("match", other),
    };
    Ok(value)
}

#[test]
fn match_vectors_replay_exactly() {
    let deviations = [
        Deviation {
            name: "D-EXTRACT-STALE-HINT",
            case_id: "model-hints-are-checked",
            step: 4,
            rewrite: |value| {
                assert_eq!(
                    *value,
                    json!({
                        "ambiguous": true,
                        "candidate_ids": ["a1a1a1a1-0000-4000-8000-000000000001", "b2b2b2b2-0000-4000-8000-000000000002"],
                        "matched": true,
                        "reason": "2 runs match equally well",
                        "reason_code": "",
                        "run_id": "a1a1a1a1-0000-4000-8000-000000000001",
                    })
                );
                *value = json!({
                    "ambiguous": false, "candidate_ids": [], "matched": false,
                    "reason": "model pointed at terminal run #e5e5e5e5",
                    "reason_code": "terminal-hint", "run_id": null,
                });
                1
            },
        },
        Deviation {
            name: "D-EXTRACT-WEEK-ANCHOR",
            case_id: "spanning-reachability-and-kinds",
            step: 6,
            rewrite: |value| {
                assert_eq!(
                    *value,
                    json!([
                        "a1a1a1a1-0000-4000-8000-000000000001",
                        "d4d4d4d4-0000-4000-8000-000000000004",
                    ])
                );
                *value = json!(["a1a1a1a1-0000-4000-8000-000000000001"]);
                1
            },
        },
    ];
    assert_eq!(
        replay_family_with("match", &deviations, |_, _| {}, replay),
        (3, 33)
    );
}
