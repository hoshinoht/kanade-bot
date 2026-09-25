//! Nudge seed pools and `compact.nudge_rewrite`: schema and precedence only.

use kanade::chat::persona::{
    CompiledPersona, MAX_NUDGE_CHARS, NudgeMood, NudgePurpose, NudgeSource, PersonaError,
    YamlIssue, check_nudge_line, fill_nudge, parse_bundle, parse_profile,
};
use kanade::chat::prompts::builtin_nudges;

use crate::support::{bundle, pid, prof, profile};

const PURPOSES: [NudgePurpose; 2] = [NudgePurpose::SelfService, NudgePurpose::RequestForm];
const MOODS: [NudgeMood; 2] = [NudgeMood::Playful, NudgeMood::Gentle];

fn pool(tag: &str) -> String {
    format!("['{tag} one', '{tag} two', '{tag} three']")
}

fn with_nudges(section: &str) -> String {
    format!("{}nudges:\n{section}", bundle("alpha", "Alpha"))
}

fn parse(section: &str) -> Result<kanade::chat::persona::Bundle, PersonaError> {
    parse_bundle(&with_nudges(section), &pid("alpha"))
}

#[test]
fn pools_resolve_profile_then_bundle_then_built_in() {
    let bundle_text = with_nudges(&format!(
        "  playful: {}\n  gentle: {}\n  request_form:\n    playful: {}\n",
        pool("b-play"),
        pool("b-gentle"),
        pool("b-req-play")
    ));
    let bundle = parse_bundle(&bundle_text, &pid("alpha")).unwrap();
    let profile_text = format!(
        "{}nudges:\n  gentle: {}\n  request_form:\n    gentle: {}\n",
        profile("style", None),
        pool("p-gentle"),
        pool("p-req-gentle")
    );
    let style = parse_profile(&profile_text, &prof("style")).unwrap();
    let compiled = CompiledPersona::compile(&bundle, Some(&style));
    let first = |purpose, mood| {
        let seeds = compiled.nudge_seeds(purpose, mood);
        (seeds.lines[0].to_owned(), seeds.source)
    };
    use NudgeMood::*;
    use NudgePurpose::*;
    assert_eq!(
        first(SelfService, Playful),
        ("b-play one".into(), NudgeSource::Bundle)
    );
    assert_eq!(
        first(SelfService, Gentle),
        ("p-gentle one".into(), NudgeSource::Profile)
    );
    assert_eq!(
        first(RequestForm, Playful),
        ("b-req-play one".into(), NudgeSource::Bundle)
    );
    assert_eq!(
        first(RequestForm, Gentle),
        ("p-req-gentle one".into(), NudgeSource::Profile)
    );

    let plain = CompiledPersona::compile(
        &parse_bundle(&crate::support::bundle("alpha", "Alpha"), &pid("alpha")).unwrap(),
        None,
    );
    for purpose in PURPOSES {
        for mood in MOODS {
            let seeds = plain.nudge_seeds(purpose, mood);
            assert_eq!(seeds.source, NudgeSource::BuiltIn);
            assert_eq!(seeds.lines, builtin_nudges(purpose, mood));
        }
    }
    // A missing gentle pool never borrows playful lines.
    let playful_only = parse(&format!("  playful: {}\n", pool("x"))).unwrap();
    let compiled = CompiledPersona::compile(&playful_only, None);
    assert_eq!(
        compiled.nudge_seeds(SelfService, Gentle).source,
        NudgeSource::BuiltIn
    );
}

#[test]
fn nudge_pools_are_strictly_validated() {
    let lines = |count: usize| {
        let items = (0..count)
            .map(|n| format!("'line {n}'"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("  playful: [{items}]\n")
    };
    assert!(parse(&lines(3)).is_ok());
    assert!(parse(&lines(20)).is_ok());
    let long = "x".repeat(MAX_NUDGE_CHARS + 1);
    let exact = "x".repeat(MAX_NUDGE_CHARS);
    assert!(parse(&format!("  gentle: ['{exact}', b, c]\n")).is_ok());
    let rejected = [
        lines(2),
        lines(21),
        format!("  playful: ['{long}', b, c]\n"),
        "  playful: ['see https://x.dev', b, c]\n".into(),
        "  playful: ['see www.example.com', b, c]\n".into(),
        "  playful: ['[here](/runs/1)', b, c]\n".into(),
        "  playful: ['hey <@123>', b, c]\n".into(),
        "  playful: ['<@&9> look', b, c]\n".into(),
        "  playful: ['go <#9>', b, c]\n".into(),
        "  playful: ['@everyone look', b, c]\n".into(),
        "  playful: ['@here look', b, c]\n".into(),
        "  playful: ['{name} look', b, c]\n".into(),
        "  playful: ['{0} look', b, c]\n".into(),
        "  playful: ['{boss!r}', b, c]\n".into(),
        // Deleting `{boss}` would assemble `{day}`; the scan must still refuse it.
        "  playful: ['{da{boss}y} look', b, c]\n".into(),
        "  playful: ['{boss}} look', b, c]\n".into(),
        "  playful: ['{{time} look', b, c]\n".into(),
        "  playful: [' padded', b, c]\n".into(),
        "  playful: ['', b, c]\n".into(),
        "  playful: [\"two\\nlines\", b, c]\n".into(),
        "  playful: []\n".into(),
        "  request_form: {}\n".into(),
        format!(
            "  request_form:\n    playful: {}\n    gentle: [a]\n",
            pool("r")
        ),
    ];
    for section in &rejected {
        assert!(
            matches!(parse(section), Err(PersonaError::Invalid(_))),
            "{section}"
        );
    }
    for section in [
        format!("  angry: {}\n", pool("a")),
        format!("  request_form:\n    angry: {}\n", pool("a")),
        format!("  self_service: {}\n", pool("a")),
    ] {
        assert_eq!(
            parse(&section).unwrap_err(),
            PersonaError::Yaml(YamlIssue::UnknownField),
            "{section}"
        );
    }
    assert!(parse(" {}\n").is_err());
    let placeholders = parse("  playful: ['{boss} on {day} at {time}', b, c]\n").unwrap();
    assert!(placeholders.nudges.is_some());
}

#[test]
fn built_in_lines_and_user_drafts_satisfy_the_schema() {
    for purpose in PURPOSES {
        for mood in MOODS {
            let lines = builtin_nudges(purpose, mood);
            assert!((3..=20).contains(&lines.len()));
            for line in lines {
                check_nudge_line(line).unwrap();
            }
        }
    }
    // User-provided Kanade drafts (workplan note, 2026-09-24).
    for line in [
        "moou... why don't you do it yourself!?",
        "Eh? You want me to change it? ...fine, here's the button, do it yourself.",
        "I already did the hard part, you know. Tweaking it is on you~",
        "Hmph. Everything you need is right here.",
        "Again? ...okay, okay. Edit it here, and don't break it this time.",
        "Ah... that didn't go well, did it. You can fix it here.",
    ] {
        check_nudge_line(line).unwrap();
    }
}

#[test]
fn fill_is_single_pass_and_literal() {
    assert_eq!(
        fill_nudge("{boss} on {day} at {time}", "Lotus", "Mon", "21:00"),
        "Lotus on Mon at 21:00"
    );
    assert_eq!(
        fill_nudge("{boss}!", "{day} {time}", "Mon", "21:00"),
        "{day} {time}!"
    );
    assert_eq!(fill_nudge("{ {x} }", "a", "b", "c"), "{ {x} }");
}

#[test]
fn nudge_rewrite_is_an_optional_compact_sibling() {
    let base = bundle("alpha", "Alpha");
    let only_nudge = format!("{base}compact:\n  nudge_rewrite: |\n    Rewrite the lead-in.\n");
    let parsed = parse_bundle(&only_nudge, &pid("alpha")).unwrap();
    let compact = parsed.compact.clone().unwrap();
    assert_eq!(compact.header_rewrite, None);
    assert_eq!(
        compact.nudge_rewrite.as_deref(),
        Some("Rewrite the lead-in.\n")
    );
    let compiled = CompiledPersona::compile(&parsed, None);
    assert_eq!(compiled.prompt_compact(), None);
    assert_eq!(compiled.nudge_rewrite(), Some("Rewrite the lead-in.\n"));
    assert!(!compiled.prompt().contains("Rewrite the lead-in."));
    for text in [
        format!("{base}compact: {{}}\n"),
        format!("{base}compact:\n  nudge_rewrite: '  '\n"),
    ] {
        assert!(
            matches!(
                parse_bundle(&text, &pid("alpha")),
                Err(PersonaError::Invalid(_))
            ),
            "{text}"
        );
    }
    // The tracked Kanade bundle carries a nudge_rewrite; its approved general
    // pools also serve request-form tips, before any built-in line.
    let root = kanade::chat::persona::PersonaRoot::open(&crate::support::tracked_dir()).unwrap();
    let kanade = root.load_bundle(&pid("kanade")).unwrap().value;
    let compiled = CompiledPersona::compile(&kanade, None);
    assert!(
        compiled
            .nudge_rewrite()
            .is_some_and(|text| text.contains("Kanade"))
    );
    let gentle = compiled.nudge_seeds(NudgePurpose::SelfService, NudgeMood::Gentle);
    assert_eq!(gentle.source, NudgeSource::Bundle);
    assert_eq!(
        gentle.lines,
        [
            "Ah... that didn't go well, did it. You can fix it here.",
            "Don't look so down... it's an easy fix. Go on, it's right here.",
            "Hmph, it's not your fault. Just adjust it here, okay?",
        ]
    );
    let playful = compiled.nudge_seeds(NudgePurpose::SelfService, NudgeMood::Playful);
    assert_eq!(playful.source, NudgeSource::Bundle);
    assert_eq!(playful.lines.len(), 3);
    for mood in MOODS {
        let request = compiled.nudge_seeds(NudgePurpose::RequestForm, mood);
        assert_eq!(request.source, NudgeSource::Bundle);
    }
}
