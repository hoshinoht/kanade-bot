use kanade::domain::time::{self, IsoError};
use serde_json::{Value, json};

use crate::support::{Outcome, instant, iso, replay_family, text, unknown_op, zone};

fn replay(op: &str, input: &Value, _fixtures: &Value) -> Outcome {
    match op {
        "to_iso" => Ok(json!(iso(input, "at").to_iso().map_err(iso_error)?)),
        "from_iso" => {
            let at = time::from_iso(text(input, "value")).map_err(iso_error)?;
            Ok(json!(
                time::to_iso(&at).map_err(|error| ("OverflowError", error.to_string()))?
            ))
        }
        "local_naive" => {
            let wall = time::local_naive(&instant(input, "at"), zone(input))
                .map_err(|error| ("OverflowError", error.to_string()))?;
            Ok(json!(time::isoformat_naive(&wall)))
        }
        other => unknown_op("timeutil", other),
    }
}

fn iso_error(error: IsoError) -> (&'static str, String) {
    let kind = match error {
        IsoError::Overflow(_) => "OverflowError",
        _ => "ValueError",
    };
    (kind, error.to_string())
}

#[test]
fn timeutil_vectors_replay_exactly() {
    replay_family("timeutil", replay);
}
