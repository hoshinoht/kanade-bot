//! Strict five-key staging and literal `{boss}` substitution.

use kanade::chat::persona::{
    CompiledPersona, MAX_RENDERED_STAGING_CHARS, MAX_STAGING_CHARS, PersonaError, Staging,
    StagingState, YamlIssue, parse_bundle, parse_profile,
};

use crate::support::{bundle, pid, prof, profile};

fn with_line(key: &str, yaml_value: &str) -> String {
    let base = bundle("alpha", "Alpha");
    let old = base
        .lines()
        .find(|line| line.starts_with(&format!("  {key}: ")))
        .unwrap()
        .to_owned();
    base.replace(&old, &format!("  {key}: {yaml_value}"))
}

fn invalid(text: &str) -> bool {
    matches!(
        parse_bundle(text, &pid("alpha")),
        Err(PersonaError::Invalid(_))
    )
}

#[test]
fn bundle_lines_follow_the_strict_rules() {
    let long = format!("'{}'", "x".repeat(MAX_STAGING_CHARS + 1));
    let rejected = [
        ("schedule", "|\n    two\n    lines"),
        ("generic", "\"tab\\there\""),
        ("guide", long.as_str()),
        ("guide_named", "'no field here'"),
        ("guide_named", "'{boss} and {boss}'"),
        ("guide_named", "'{boss} {0}'"),
        ("guide_named", "'{boss!r}'"),
        ("guide_named", "'{{boss}}'"),
        ("guide_named", "'{{boss}boss}'"),
        ("guide_named", "'{bo{boss}ss}'"),
        ("schedule", "'checking {boss}'"),
        ("write", "'{name} writes'"),
        ("generic", "'odd } brace'"),
        ("generic", "'hi <@123>'"),
        ("generic", "'hi <@!123>'"),
        ("generic", "'hi <@&5>'"),
        ("generic", "'see <#9>'"),
        ("generic", "'ping @everyone'"),
        ("generic", "'ping @Here'"),
    ];
    for (key, value) in rejected {
        assert!(invalid(&with_line(key, value)), "{key}: {value}");
    }
    let exact = format!("'{}'", "y".repeat(MAX_STAGING_CHARS));
    assert!(parse_bundle(&with_line("guide", &exact), &pid("alpha")).is_ok());
    let dashed = bundle("alpha", "Alpha").replace("  guide_named:", "  guide-named:");
    assert_eq!(
        parse_bundle(&dashed, &pid("alpha")).unwrap_err(),
        PersonaError::Yaml(YamlIssue::UnknownField)
    );
    let missing = bundle("alpha", "Alpha").replace("  write: alpha write\n", "");
    assert_eq!(
        parse_bundle(&missing, &pid("alpha")).unwrap_err(),
        PersonaError::Yaml(YamlIssue::MissingField)
    );
    for scalar in ["5", "true", "1.5", "null"] {
        assert!(
            parse_bundle(&with_line("write", scalar), &pid("alpha")).is_err(),
            "{scalar}"
        );
    }
    assert!(parse_bundle(&with_line("write", "'5'"), &pid("alpha")).is_ok());
}

#[test]
fn profile_overrides_follow_the_same_rules() {
    for line in ["'x @everyone'", "'{boss}'", "|\n    a\n    b"] {
        let text = profile("style", Some(line));
        assert!(
            matches!(
                parse_profile(&text, &prof("style")),
                Err(PersonaError::Invalid(_))
            ),
            "{line}"
        );
    }
    let text = profile("style", None).replace(
        "prompt: |",
        "staging:\n  guide_named: 'no field'\nprompt: |",
    );
    assert!(parse_profile(&text, &prof("style")).is_err());
}

#[test]
fn compiled_lines_are_stripped_and_inherited() {
    let text = with_line("generic", "|\n    padded line  ");
    let parsed = parse_bundle(&text, &pid("alpha")).unwrap();
    assert_eq!(parsed.staging.generic, "padded line  \n");
    let style =
        parse_profile(&profile("style", Some("'  own generic  '")), &prof("style")).unwrap();
    let alone = CompiledPersona::compile(&parsed, None);
    assert_eq!(alone.staging_lines().generic, "padded line");
    let merged = CompiledPersona::compile(&parsed, Some(&style));
    assert_eq!(
        merged.staging_lines(),
        &Staging {
            schedule: "alpha schedule".into(),
            guide: "alpha guide".into(),
            guide_named: "{boss} alpha guide".into(),
            write: "alpha write".into(),
            generic: "own generic".into(),
        }
    );
    assert_eq!(
        merged.staging_lines().line(StagingState::Write),
        "alpha write"
    );
    assert_eq!(
        merged.staging_lines().line(StagingState::Generic),
        "own generic"
    );
}

#[test]
fn boss_substitution_is_literal_and_bounded() {
    let staging = Staging {
        schedule: "s".into(),
        guide: "Reading notes…".into(),
        guide_named: "{boss}? Pulling notes.".into(),
        write: "w".into(),
        generic: "g".into(),
    };
    assert_eq!(staging.guide_named_for("Lotus"), "Lotus? Pulling notes.");
    assert_eq!(
        staging.guide_named_for("{boss} {0} {name}"),
        "{boss} {0} {name}? Pulling notes."
    );
    assert_eq!(
        staging.guide_named_for(" Hard Lucid "),
        "Hard Lucid? Pulling notes."
    );
    for unsafe_name in ["", "  ", "<@1>", "@everyone", "two\nlines"] {
        assert_eq!(staging.guide_named_for(unsafe_name), "Reading notes…");
    }
    let near = "b".repeat(MAX_RENDERED_STAGING_CHARS - "? Pulling notes.".chars().count());
    assert_eq!(
        staging.guide_named_for(&near).chars().count(),
        MAX_RENDERED_STAGING_CHARS
    );
    let over = format!("{near}b");
    assert_eq!(staging.guide_named_for(&over), "Reading notes…");
    let joined = Staging {
        guide_named: "<{boss}".into(),
        ..staging
    };
    assert_eq!(joined.guide_named_for("@x"), joined.guide);
}

#[test]
fn tracked_kanade_staging_is_valid_and_substitutes() {
    let root = kanade::chat::persona::PersonaRoot::open(&crate::support::tracked_dir()).unwrap();
    let kanade = root.load_bundle(&pid("kanade")).unwrap().value;
    let compiled = CompiledPersona::compile(&kanade, None);
    assert_eq!(
        compiled.staging_lines().guide_named_for("Lotus"),
        "Lotus? Eh…? Fine, serious mode. Pulling the notes so we can do this on tempo instead of improvising the clear."
    );
}
