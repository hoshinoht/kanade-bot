//! Replays the frozen v4 chat vectors owned by slice C1 (gate, authority,
//! participants, tool schemas, read tools, proposals, sanitize) and covers
//! v5's dynamic tool bundles and identity handling at the dispatcher. The
//! `loop` and `context` families belong to slice C2.

mod authority;
mod bundles;
mod gate;
mod identity;
mod propose;
mod read_tools;
mod sanitize;
mod support;
mod tool_schemas;
mod world;

// Pinned clock/ids and vector helpers shared with the scheduler target.
#[path = "../common/mod.rs"]
mod common;

const REPLAYED: [&str; 7] = [
    "gate",
    "authority",
    "participants",
    "tool_schemas",
    "read_tools",
    "propose",
    "sanitize",
];

#[test]
fn index_lists_the_replayed_families_with_their_schemas() {
    let index = support::load("index.json");
    assert_eq!(index["schema_version"], "v5-chat-index-v1");
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
