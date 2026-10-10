//! The silent staging-line selector, ported from v4 `progress.placeholder_for`
//! and `strategy.route_strategy_intent` (v4 `tests/test_chat_progress.py`,
//! `test_chat_strategy.py`, git history up to `487c4ed`) over v4's shipped
//! catalog, which `boss/bosses.yaml` keeps byte-identical.
//!
//! Named deviation `D-STAGING-WORD-CLASS`: v4's `\w` (in the how-to cue and
//! custom-emoji markup) is Python's `str.isalnum()` plus `_`; v5 uses Rust's
//! Unicode word class, which differs only for rare non-decimal numerics such
//! as `²`. Python's `\s` is matched exactly.

use std::path::Path;

use kanade::chat::persona::Staging;
use kanade::chat::pilot::staging_line;
use kanade::domain::catalog::BossTable;
use kanade::infrastructure::files::load_catalog;
use serde_json::json;

use crate::wire::kanade;
use crate::world::catalog;

/// v4's built-in `DEFAULT_LINES`.
fn v4_lines() -> Staging {
    Staging {
        schedule: "Checking your runs…".into(),
        guide: "Reading checked-in notes…".into(),
        guide_named: "Reading checked-in notes for {boss}…".into(),
        write: "Drafting the proposal card…".into(),
        generic: "Thinking…".into(),
    }
}

fn shipped() -> BossTable {
    load_catalog(&Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml"))
        .expect("shipped catalog")
}

fn line(text: &str) -> String {
    staging_line(text, &shipped(), &v4_lines())
}

#[test]
fn v4_progress_copy_cases() {
    let lines = v4_lines();
    assert_eq!(line("@bot what's on tonight?"), lines.schedule);
    assert_eq!(
        line("@bot how to beat fa"),
        "Reading checked-in notes for FA…"
    );
    assert_eq!(line("@bot move hstar to wed"), lines.write);
    assert_eq!(line("@bot hello"), lines.generic);
    assert_eq!(line("   "), lines.generic);
    assert_eq!(line(""), lines.generic);
}

#[test]
fn rendered_lines_never_mention() {
    for text in [
        "@bot how to beat fa",
        "<@&1543538821671424092> guide for FA, Kalos",
        "<@1001> tips for <@&9> and FA",
    ] {
        let rendered = line(text);
        assert!(!rendered.contains("<@"), "{rendered}");
        assert!(!rendered.contains("@everyone") && !rendered.contains("@here"));
    }
}

#[test]
fn profile_lines_replace_the_defaults() {
    let custom = Staging {
        schedule: "Custom schedule…".into(),
        generic: "Custom generic…".into(),
        ..v4_lines()
    };
    let table = shipped();
    assert_eq!(
        staging_line("@bot what's on tonight?", &table, &custom),
        "Custom schedule…"
    );
    assert_eq!(
        staging_line("@bot hello", &table, &custom),
        "Custom generic…"
    );
}

/// v4 `test_route_strategy_resolves_aliases_and_preserves_difficulty`: every
/// resolved route names its bosses' short names, in order, once each.
#[test]
fn resolved_strategy_questions_name_their_bosses() {
    let cases = [
        ("how to beat fa", "FA"),
        ("how do we clear HFA?", "FA"),
        ("how do we handle HFA?", "FA"),
        ("how should I approach FA?", "FA"),
        ("how to beat FA, please?", "FA"),
        ("guide for The First Adversary", "FA"),
        ("Kalos mechanics", "Kalos"),
        ("what attacks does FA have?!", "FA"),
        ("what moves should I watch for in FA?", "FA"),
        ("what should I watch out for in FA?", "FA"),
        ("what mechanics and requirements does FA have?", "FA"),
        ("tips for FA, HFA", "FA"),
        ("strategy for HFA and Extreme Kalos", "FA, Kalos"),
        ("strategy for HFA + Extreme Kalos", "FA, Kalos"),
        ("guide for FA vs Kalos", "FA, Kalos"),
        ("tips for FA; Kalos", "FA, Kalos"),
        ("guide for \"FA\", \"Kalos\"", "FA, Kalos"),
        ("beat FA and dodge", "FA"),
    ];
    for (text, names) in cases {
        assert_eq!(
            line(text),
            format!("Reading checked-in notes for {names}…"),
            "{text}"
        );
    }
}

/// v4 `test_schedule_wording_without_a_strong_cue_is_not_strategy`.
#[test]
fn schedule_wording_is_not_strategy() {
    let lines = v4_lines();
    assert_eq!(line("when are we clearing HFA?"), lines.generic);
    assert_eq!(line("how do I schedule HFA?"), lines.schedule);
    assert_eq!(line("move HFA to Tuesday"), lines.write);
}

/// v4's unresolved routes (no boss, an unknown or partial target list, more
/// than three bosses, a role mention that must not split targets) and
/// conflicting difficulties all show the plain guide line.
#[test]
fn unresolved_strategy_questions_show_the_guide_line() {
    for text in [
        "strategy",
        "tips for Zakum",
        "tips for Lotus, Damien",
        "tips for Lotus and Damien",
        "guide for FA, Kalos, Seren, and Lotus",
        "<@&1543538821671424092>  guide for FA, Kalos, Seren, and Lotus",
        "tips for HFA, Extreme FA",
    ] {
        assert_eq!(line(text), v4_lines().guide, "{text}");
    }
}

#[test]
fn an_unsafe_named_render_falls_back_to_the_guide_line() {
    let long = Staging {
        guide_named: format!("{{boss}} {}", "x".repeat(295)),
        ..v4_lines()
    };
    let table = shipped();
    assert_eq!(
        staging_line("how to beat fa", &table, &long)
            .chars()
            .count(),
        298,
        "one short name still fits the 300-character budget"
    );
    assert_eq!(
        staging_line("guide for FA, Kalos", &table, &long),
        long.guide,
        "the joined names push the render over budget"
    );
}

#[test]
fn the_tracked_persona_lines_are_used_after_compilation() {
    let persona = kanade();
    let lines = persona.staging_lines();
    let table = shipped();
    assert_eq!(
        staging_line("how to beat fa", &table, lines),
        lines.guide_named.replacen("{boss}", "FA", 1)
    );
    assert_eq!(staging_line("tips for Zakum", &table, lines), lines.guide);
    assert_eq!(staging_line("cancel tonight", &table, lines), lines.write);
    assert_eq!(staging_line("my runs?", &table, lines), lines.schedule);
    assert_eq!(staging_line("hi there", &table, lines), lines.generic);
}

#[test]
fn invented_bosses_resolve_only_through_the_catalog() {
    let table = catalog(&json!({
        "difficulties": [
            { "prefix": "n", "label": "Normal" },
            { "prefix": "h", "label": "Hard" },
        ],
        "bosses": [
            { "short": "Quillmaw", "full": "Quillmaw the Ink Tyrant",
              "aliases": ["quill"], "difficulties": ["n", "h"] },
            { "short": "Vesperine", "full": "Vesperine",
              "aliases": ["vesper"], "difficulties": ["n"] },
        ],
    }));
    let lines = v4_lines();
    let at = |text: &str| staging_line(text, &table, &lines);
    assert_eq!(
        at("dodge tips for hard quill and vesper"),
        "Reading checked-in notes for Quillmaw, Vesperine…"
    );
    assert_eq!(
        at("how do we clear Quillmaw the Ink Tyrant?"),
        "Reading checked-in notes for Quillmaw…"
    );
    assert_eq!(at("tips for FA"), lines.guide, "not in this catalog");
    assert_eq!(at("move quill to fri"), lines.write);
}
