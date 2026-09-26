//! Replays the frozen v4 extraction vectors: the pure rules (gate, window,
//! resolve, match, merge) and schema, prompt and burst planning (parse,
//! prompt, plan), and commit through the v5 proposal path. `pipeline` and
//! `rescan` drive the orchestration over pipeline-level fakes (`fakes`).
//! `redirect` and `nudge` cover the v5 self-service redirect and its persona
//! nudges (no v4 vectors); `self_service` wires them through the governed
//! rewrite and the pipeline. `cards` replays the proposal card text.

mod cards;
mod commit;
mod fakes;
mod gate;
mod masking;
mod matching;
mod merge;
mod nudge;
mod parse;
mod pipeline;
mod plan;
mod prompt;
mod redirect;
mod rescan;
mod resolve;
mod self_service;
mod session;
mod shaping;
mod support;
mod window;

// Pinned clock/ids and v4-shaped snapshots shared with the scheduler target.
#[path = "../common/mod.rs"]
mod common;

const REPLAYED: [&str; 10] = [
    "gate", "window", "resolve", "match", "merge", "prompt", "parse", "plan", "commit", "cards",
];

#[test]
fn index_lists_the_replayed_families_with_their_schemas() {
    let index = support::load("index.json");
    assert_eq!(index["schema_version"], "v5-extract-index-v1");
    let families = index["families"].as_array().expect("families");
    for family in REPLAYED {
        let entry = families
            .iter()
            .find(|entry| entry["family"] == family)
            .unwrap_or_else(|| panic!("index lacks {family}"));
        assert_eq!(entry["schema"], format!("{family}.schema.json"));
        assert_eq!(entry["vector"], format!("{family}.json"));
    }
}
