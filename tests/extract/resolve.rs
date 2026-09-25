use kanade::extract::resolve::{self, PM_CUTOFF};
use serde_json::{Value, json};

use crate::support::{
    Outcome, clock_iso, instant, opt_text, replay_family, resolved_json, text, unknown_op, zone,
};

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
