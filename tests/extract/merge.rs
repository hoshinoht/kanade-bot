use kanade::extract::merge;
use serde_json::{Value, json};

use crate::support::{
    Outcome, amendment, amendment_json, replay_family, strings, text, unknown_op,
};

fn replay(_input: &Value, step: &Value) -> Outcome {
    match text(&step["op"]) {
        "merge" => {
            let amendments: Vec<_> = step["amendments"]
                .as_array()
                .expect("amendments")
                .iter()
                .map(amendment)
                .collect();
            let existing: Vec<Vec<String>> = step["existing_bosses"]
                .as_array()
                .expect("existing_bosses")
                .iter()
                .map(strings)
                .collect();
            let merged = merge::merge(&amendments, &strings(&step["message_order"]), &existing);
            Ok(json!(merged.iter().map(amendment_json).collect::<Vec<_>>()))
        }
        other => unknown_op("merge", other),
    }
}

#[test]
fn merge_vectors_replay_exactly() {
    assert_eq!(replay_family("merge", replay), (4, 10));
}
