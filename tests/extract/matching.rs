use kanade::domain::schedule::Run;
use kanade::extract::AmendmentKind;
use kanade::extract::matching::{self, MatchResult};
use serde_json::{Value, json};

use crate::support::{
    Outcome, amendment, date, opt_text, replay_family, run, select, strings, text, unknown_op, zone,
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
            run_ids(&matching::reachable(&select(&pool, &step["runs"]), day, tz))
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
    assert_eq!(replay_family("match", replay), (3, 33));
}
