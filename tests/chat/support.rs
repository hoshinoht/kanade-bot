//! The synthetic chat world (`tests/fixtures/chat/world.json`): roster,
//! channels, pilot settings, catalog, seeded runs, pinned clock and id
//! sequence, and the model's capability flags.

use std::{fs, path::PathBuf};

use serde_json::Value;

pub fn world_input() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chat/world.json");
    let text = fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{path:?}: {error}"))
}
