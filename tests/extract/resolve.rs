use kanade::extract::resolve::{self, PM_CUTOFF, Resolved};
use serde_json::{Value, json};

use crate::support::{
    Outcome, clock_iso, instant, opt_text, replay_family, text, unknown_op, zone,
};

fn resolved_json(value: &Resolved) -> Value {
    json!({
        "day": value.day.map(|day| day.to_string()),
        "clock": value.clock.map(clock_iso),
        "at": value.at.map(|at| at.isoformat()),
        "assumed_pm": value.assumed_pm,
        "known": value.known(),
    })
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let tz = zone(input);
    let time_ref = || opt_text(&step["time_ref"]);
    let value = match text(&step["op"]) {
        "pm_cutoff" => json!(PM_CUTOFF),
        "parse_clock" => match resolve::parse_clock(time_ref()) {
            Some((clock, assumed_pm)) => {
                json!({ "clock": clock_iso(clock), "assumed_pm": assumed_pm })
            }
            None => Value::Null,
        },
        "resolve" => {
            let found = resolve::resolve(
                opt_text(&step["day_ref"]),
                time_ref(),
                &instant(&step["anchor"]),
                tz,
            )
            .expect("in range");
            resolved_json(&found)
        }
        other => unknown_op("resolve", other),
    };
    Ok(value)
}

#[test]
fn resolve_vectors_replay_exactly() {
    assert_eq!(replay_family("resolve", replay), (4, 63));
}
