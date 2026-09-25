//! Replays the frozen v4 scheduler and reminder vectors through the v5
//! scheduler service and in-memory store, plus store conformance and invariants.

mod attendance_v5;
mod draft_service;
mod drafts;
mod invariants;
mod mutations_family;
mod mutations_v5;
mod proposal_follow_ups;
mod proposals;
mod reminders_family;
mod requests;
mod scheduler_family;

// Vector loading, pinned clock/ids and snapshots shared with the notify target.
#[path = "../common/mod.rs"]
mod common;

use kanade::infrastructure::store::{MemoryScheduleStore, conformance};

/// Every family this target replays.
const REPLAYED: [(&str, &str, &str); 3] = [
    ("scheduler", "schema.json", "scheduler.json"),
    ("reminders", "reminders.schema.json", "reminders.json"),
    ("mutations", "mutations.schema.json", "mutations.json"),
];

#[test]
fn index_lists_the_replayed_families_with_their_schemas() {
    let index = common::load("index.json");
    let families = index["families"].as_array().expect("families");
    for (family, schema, vector) in REPLAYED {
        let entry = families
            .iter()
            .find(|entry| entry["family"] == family)
            .unwrap_or_else(|| panic!("index lacks {family}"));
        assert_eq!(entry["schema"], schema, "{family}");
        assert_eq!(entry["vector"], vector, "{family}");
    }
}

#[test]
fn vector_files_satisfy_their_schemas() {
    for (family, schema, vector) in REPLAYED {
        let schema = common::load(schema);
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let file = common::load(vector);
        let errors: Vec<String> = validator
            .iter_errors(&file)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{family}: {errors:?}");
    }
}

#[tokio::test]
async fn memory_store_passes_the_store_conformance_suite() {
    conformance::run_suite(async || MemoryScheduleStore::new()).await;
}
