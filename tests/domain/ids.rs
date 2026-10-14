use kanade::domain::ids::{self, IdError};
use serde_json::{Value, json};

use crate::support::{Outcome, replay_family, text, unknown_op};

fn replay(op: &str, input: &Value, _fixtures: &Value) -> Outcome {
    match op {
        "canonical" => Ok(json!(ids::canonical(text(input, "value")))),
        "short_id" => Ok(json!(ids::short_id(text(input, "value")))),
        "tag" => Ok(json!(ids::tag(text(input, "value")))),
        "resolve_id" => {
            let candidates: Vec<&str> = input["candidates"]
                .as_array()
                .expect("candidates array")
                .iter()
                .map(|candidate| candidate.as_str().expect("candidate string"))
                .collect();
            let resolved = ids::resolve_id(text(input, "text"), candidates).map_err(|error| {
                let kind = match error {
                    IdError::TooShort { .. } => "IdTooShort",
                    IdError::NotFound { .. } => "IdNotFound",
                    IdError::Ambiguous { .. } => "IdAmbiguous",
                };
                (kind, error.to_string())
            })?;
            Ok(json!(resolved))
        }
        other => unknown_op("ids", other),
    }
}

#[test]
fn ids_vectors_replay_exactly() {
    replay_family("ids", replay);
}
