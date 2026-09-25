use kanade::chat::persona::{
    PersonaError, Staging, StagingOverride, YamlIssue, parse_bundle, parse_catalog, parse_profile,
};

use crate::support::{bundle, catalog, pid, prof, profile, tracked};

fn yaml(issue: YamlIssue) -> PersonaError {
    PersonaError::Yaml(issue)
}

#[test]
fn tracked_kanade_bundle_keeps_block_scalars() {
    let parsed = parse_bundle(&tracked("bundles/kanade.yaml"), &pid("kanade")).unwrap();
    assert!(
        parsed
            .identity
            .starts_with("# Persona: OtonoseKanade\n\n## Identity\n\n")
    );
    assert!(parsed.identity.ends_with("these boundaries.\n"));
    assert!(!parsed.identity.ends_with("\n\n"));
    assert!(
        parsed
            .prompt
            .contains("coordination matter.\n\nUse her speech")
    );
    assert_eq!(parsed.voice, None);
    assert!(parsed.staging.guide_named.starts_with("{boss}? Eh…?"));
}

#[test]
fn tracked_kanade_header_rewrite_is_the_approved_bytes() {
    const SHA256: &str = "dba87c180e9b52dfbe2a1b168c455c0d81b08864c6727f5c8c292853dc374dfc";
    let root = kanade::chat::persona::PersonaRoot::open(&crate::support::tracked_dir()).unwrap();
    let compact = root.load_bundle(&pid("kanade")).unwrap().value.compact;
    let text = compact
        .expect("tracked Kanade carries compact")
        .header_rewrite;
    let digest = ring::digest::digest(&ring::digest::SHA256, text.as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(text.len(), 408);
    assert!(text.ends_with(".\n") && !text.ends_with("\n\n"));
    assert_eq!(digest, SHA256);
}

#[test]
fn compact_is_optional_for_other_bundles() {
    let parsed = parse_bundle(&bundle("alpha", "Alpha"), &pid("alpha")).unwrap();
    assert_eq!(parsed.compact, None);
}

#[test]
fn tracked_examples_parse() {
    let parsed = parse_catalog(&tracked("catalog.example.yaml")).unwrap();
    assert_eq!(parsed.default_id(), &pid("kanade"));
    let example = parse_profile(&tracked("profiles/example.yaml"), &prof("example")).unwrap();
    assert_eq!(
        example.staging.generic.as_deref(),
        Some("Processing request. Stand by.")
    );
    assert_eq!(example.staging.schedule, None);
}

#[test]
fn block_scalar_internal_newlines_are_preserved() {
    let text = bundle("alpha", "Alpha").replace(
        "  prompt: |\n    Synthetic behaviour for alpha.\n",
        "  prompt: |\n    first\n\n\n    second\n      indented\n",
    );
    let parsed = parse_bundle(&text, &pid("alpha")).unwrap();
    assert_eq!(parsed.prompt, "first\n\n\nsecond\n  indented\n");
}

#[test]
fn unknown_keys_are_rejected_at_every_level() {
    let base_catalog = catalog("alpha", &[("alpha", &[])]);
    let cases = [
        base_catalog.replace("default:", "extra: 1\ndefault:"),
        base_catalog.replace("    aliases:", "    path: x.yaml\n    aliases:"),
    ];
    for text in cases {
        assert_eq!(
            parse_catalog(&text).unwrap_err(),
            yaml(YamlIssue::UnknownField)
        );
    }
    let base = bundle("alpha", "Alpha");
    let cases = [
        base.replace("identity: |", "label: x\nidentity: |"),
        base.replace("  voice:", "  examples: x\n  voice:"),
        base.replace("  generic: alpha generic", "  generic: g\n  thinking: t"),
        base.replace("  guide_named:", "  guide-named:"),
        format!("{base}compact:\n  header_rewrite: h\n  reminder: r\n"),
    ];
    for text in cases {
        assert_eq!(
            parse_bundle(&text, &pid("alpha")).unwrap_err(),
            yaml(YamlIssue::UnknownField),
            "{text}"
        );
    }
    let cases = [
        profile("style", None).replace("label:", "published: true\nlabel:"),
        profile("style", Some("g")).replace("  generic: g", "  generic: g\n  thinking: t"),
    ];
    for text in cases {
        assert_eq!(
            parse_profile(&text, &prof("style")).unwrap_err(),
            yaml(YamlIssue::UnknownField)
        );
    }
}

#[test]
fn duplicate_keys_are_rejected_at_every_level() {
    let text = catalog("alpha", &[("alpha", &[])])
        .replace("default: alpha", "default: alpha\ndefault: alpha");
    assert_eq!(
        parse_catalog(&text).unwrap_err(),
        yaml(YamlIssue::DuplicateKey)
    );
    let base = bundle("alpha", "Alpha");
    let cases = [
        base.replace("id: alpha", "id: alpha\nid: alpha"),
        base.replace("  write: alpha write", "  write: a\n  write: b"),
        base.replace("  voice:", "  prompt: first\n  voice:"),
    ];
    for text in cases {
        assert_eq!(
            parse_bundle(&text, &pid("alpha")).unwrap_err(),
            yaml(YamlIssue::DuplicateKey),
            "{text}"
        );
    }
    let text = profile("style", Some("g")).replace("  generic: g", "  generic: g\n  generic: h");
    assert_eq!(
        parse_profile(&text, &prof("style")).unwrap_err(),
        yaml(YamlIssue::DuplicateKey)
    );
}

#[test]
fn incomplete_bundle_staging_is_rejected() {
    let text = bundle("alpha", "Alpha").replace("  generic: alpha generic\n", "");
    assert_eq!(
        parse_bundle(&text, &pid("alpha")).unwrap_err(),
        yaml(YamlIssue::MissingField)
    );
    let text = bundle("alpha", "Alpha").replace("  generic: alpha generic", "  generic: '  '");
    assert!(matches!(
        parse_bundle(&text, &pid("alpha")).unwrap_err(),
        PersonaError::Invalid(_)
    ));
}

#[test]
fn yaml_documents_anchors_and_tags_are_rejected() {
    let base = bundle("alpha", "Alpha");
    let cases = [
        format!("{base}---\n{base}"),
        base.replace(
            "  schedule: alpha schedule",
            "  schedule: &line alpha schedule",
        )
        .replace("  guide: alpha guide", "  guide: *line"),
        base.replace("  schedule: alpha schedule", "  schedule: !custom alpha"),
    ];
    for text in cases {
        assert!(matches!(
            parse_bundle(&text, &pid("alpha")).unwrap_err(),
            PersonaError::Yaml(_)
        ));
    }
}

#[test]
fn unsafe_ids_are_rejected() {
    for id in [
        "",
        "Kanade",
        "-lead",
        "a_b",
        "a/b",
        "../x",
        "x.yaml",
        "a b",
        &"a".repeat(51),
    ] {
        assert_eq!(
            kanade::chat::persona::PersonaId::parse(id),
            Err(PersonaError::UnsafeId),
            "{id:?}"
        );
        assert_eq!(
            kanade::chat::persona::ProfileId::parse(id),
            Err(PersonaError::UnsafeId)
        );
    }
    assert!(kanade::chat::persona::PersonaId::parse(&"a".repeat(50)).is_ok());
    let text = catalog("alpha", &[("alpha", &[]), ("Beta", &[])]);
    assert_eq!(parse_catalog(&text).unwrap_err(), PersonaError::UnsafeId);
    let text = catalog("../alpha", &[("alpha", &[])]);
    assert_eq!(parse_catalog(&text).unwrap_err(), PersonaError::UnsafeId);
}

#[test]
fn unsafe_aliases_are_rejected() {
    for alias in ["", ".", "..", "a/b", "a\\\\b", "/abs", "tab\\tx"] {
        let text = catalog("alpha", &[("alpha", &[])])
            .replace("aliases: []", &format!("aliases: [\"{alias}\"]"));
        assert_eq!(
            parse_catalog(&text).unwrap_err(),
            PersonaError::UnsafeAlias,
            "{alias:?}"
        );
    }
    let text = catalog("alpha", &[("alpha", &["persona.md", "old name"])]);
    assert!(parse_catalog(&text).is_ok());
}

#[test]
fn ids_and_aliases_must_not_collide() {
    let cases = [
        catalog("alpha", &[("alpha", &["alpha"])]),
        catalog("alpha", &[("alpha", &["beta"]), ("beta", &[])]),
        catalog("alpha", &[("alpha", &["old.md"]), ("beta", &["old.md"])]),
        catalog("alpha", &[("alpha", &["old.md", "old.md"])]),
        catalog("alpha", &[("alpha", &[]), ("alpha", &[])]),
    ];
    for text in cases {
        assert!(
            matches!(parse_catalog(&text).unwrap_err(), PersonaError::Invalid(_)),
            "{text}"
        );
    }
}

#[test]
fn catalog_semantics_are_strict() {
    let cases = [
        catalog("gamma", &[("alpha", &[])]),
        catalog("alpha", &[("alpha", &[])]).replace("schema_version: 1", "schema_version: 2"),
        "schema_version: 1\ndefault: alpha\npersonas: []\n".to_owned(),
        catalog("alpha", &[("alpha", &[])]).replace("label: Label alpha", "label: '  '"),
    ];
    for text in cases {
        assert!(
            matches!(parse_catalog(&text).unwrap_err(), PersonaError::Invalid(_)),
            "{text}"
        );
    }
}

#[test]
fn canonical_ids_resolve_aliases_only_through_the_boundary() {
    let parsed = parse_catalog(&catalog(
        "alpha",
        &[("alpha", &["persona.md"]), ("beta", &[])],
    ))
    .unwrap();
    assert_eq!(parsed.canonicalize("persona.md"), Some(&pid("alpha")));
    assert_eq!(parsed.canonicalize("beta"), Some(&pid("beta")));
    assert_eq!(parsed.canonicalize("missing.md"), None);
}

#[test]
fn file_identity_and_line_rules_are_enforced() {
    assert!(matches!(
        parse_bundle(&bundle("alpha", "Alpha"), &pid("beta")).unwrap_err(),
        PersonaError::Invalid(_)
    ));
    assert!(matches!(
        parse_profile(&profile("style", None), &prof("other")).unwrap_err(),
        PersonaError::Invalid(_)
    ));
    let text = bundle("alpha", "Alpha").replace(
        "  voice: Synthetic alpha voice.",
        "  voice: |\n    two\n    lines",
    );
    assert!(matches!(
        parse_bundle(&text, &pid("alpha")).unwrap_err(),
        PersonaError::Invalid(_)
    ));
    let text =
        profile("style", None).replace("prompt: |\n  Synthetic profile style.\n", "prompt: ''\n");
    assert!(matches!(
        parse_profile(&text, &prof("style")).unwrap_err(),
        PersonaError::Invalid(_)
    ));
    let text = format!(
        "{}compact:\n  header_rewrite: |\n    Rewrite.\n",
        bundle("alpha", "Alpha")
    );
    let parsed = parse_bundle(&text, &pid("alpha")).unwrap();
    assert_eq!(parsed.compact.unwrap().header_rewrite, "Rewrite.\n");
}

#[test]
fn profile_staging_overrides_only_its_own_keys() {
    let base = Staging {
        schedule: "s".into(),
        guide: "g".into(),
        guide_named: "{boss} n".into(),
        write: "w".into(),
        generic: "x".into(),
    };
    let partial = StagingOverride {
        guide: Some("profile guide".into()),
        ..StagingOverride::default()
    };
    let merged = partial.apply(&base);
    assert_eq!(merged.guide, "profile guide");
    assert_eq!(
        Staging {
            guide: "g".into(),
            ..merged
        },
        base
    );
}
