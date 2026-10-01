//! Replays every frozen v4 chat vector family (gate, authority,
//! participants, tool schemas, read tools, proposals, sanitize, context and
//! the loop) and covers v5's dynamic tool bundles, the governed question loop
//! and the chat log.

mod answer;
mod authority;
mod budget;
mod bundles;
mod context;
mod gate;
mod live_switch;
mod looping;
mod model;
mod pilot;
mod propose;
mod read_tools;
mod sanitize;
mod slow_store;
mod staging;
mod support;
mod tool_schemas;
mod wire;
mod world;

// Pinned clock/ids and vector helpers shared with the scheduler target.
#[path = "../common/mod.rs"]
mod common;

const REPLAYED: [&str; 9] = [
    "gate",
    "authority",
    "participants",
    "tool_schemas",
    "read_tools",
    "propose",
    "sanitize",
    "loop",
    "context",
];

#[test]
fn index_lists_the_replayed_families_with_their_schemas() {
    let index = support::load("index.json");
    assert_eq!(index["schema_version"], "v5-chat-index-v1");
    let families = index["families"].as_array().expect("families");
    assert_eq!(families.len(), REPLAYED.len(), "every family is replayed");
    for family in REPLAYED {
        let entry = families
            .iter()
            .find(|entry| entry["family"] == family)
            .unwrap_or_else(|| panic!("index lacks {family}"));
        assert_eq!(entry["schema"], format!("{family}.schema.json"));
        assert_eq!(entry["vector"], format!("{family}.json"));
    }
}
