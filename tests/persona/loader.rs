use std::{fs, os::unix::fs::symlink, process::Command};

use kanade_voice::persona::{MAX_PROFILE_FILES, PersonaError, PersonaRoot};

use crate::support::{Fixture, bundle, catalog, pid, prof, profile};

#[test]
fn bundle_paths_derive_from_ids_and_report_basename_digest() {
    let fixture = Fixture::new();
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    let loaded = fixture.root().load_bundle(&pid("alpha")).unwrap();
    let expected = ring::digest::digest(&ring::digest::SHA256, bundle("alpha", "Alpha").as_bytes())
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(loaded.source.basename, "alpha.yaml");
    assert_eq!(loaded.source.sha256, expected);
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Changed"));
    assert_ne!(
        fixture.root().load_bundle(&pid("alpha")).unwrap().source,
        loaded.source
    );
}

#[test]
fn symlinks_are_rejected_for_root_dirs_and_files() {
    let fixture = Fixture::new();
    let outside = fixture.outside();
    fs::write(outside.join("alpha.yaml"), bundle("alpha", "Alpha")).unwrap();
    fs::write(
        outside.join("catalog.yaml"),
        catalog("alpha", &[("alpha", &[])]),
    )
    .unwrap();

    symlink(
        outside.join("alpha.yaml"),
        fixture.path("bundles/alpha.yaml"),
    )
    .unwrap();
    symlink(outside.join("catalog.yaml"), fixture.path("catalog.yaml")).unwrap();
    let root = fixture.root();
    assert_eq!(
        root.load_bundle(&pid("alpha")).unwrap_err(),
        PersonaError::Symlink
    );
    assert_eq!(root.load_catalog().unwrap_err(), PersonaError::Symlink);

    // An inside target is still a symlink and still rejected.
    fixture.write("bundles/beta.yaml", bundle("beta", "Beta"));
    symlink(
        fixture.path("bundles/beta.yaml"),
        fixture.path("bundles/gamma.yaml"),
    )
    .unwrap();
    assert_eq!(
        root.load_bundle(&pid("gamma")).unwrap_err(),
        PersonaError::Symlink
    );

    fs::remove_dir_all(fixture.path("profiles")).unwrap();
    symlink(&outside, fixture.path("profiles")).unwrap();
    fs::write(outside.join("style.yaml"), profile("style", None)).unwrap();
    assert_eq!(
        root.load_profile(&prof("style")).unwrap_err(),
        PersonaError::Symlink
    );
    assert_eq!(root.load_profiles().unwrap_err(), PersonaError::Symlink);

    let linked_root = fixture.outside().join("linked-root");
    symlink(&fixture.dir, &linked_root).unwrap();
    assert_eq!(
        PersonaRoot::open(&linked_root).unwrap_err(),
        PersonaError::Symlink
    );
}

#[test]
fn symlinked_bundles_directory_is_rejected() {
    let fixture = Fixture::new();
    let outside = fixture.outside();
    fs::write(outside.join("alpha.yaml"), bundle("alpha", "Alpha")).unwrap();
    fs::remove_dir_all(fixture.path("bundles")).unwrap();
    symlink(&outside, fixture.path("bundles")).unwrap();
    assert_eq!(
        fixture.root().load_bundle(&pid("alpha")).unwrap_err(),
        PersonaError::Symlink
    );
}

#[test]
fn non_regular_files_are_rejected() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.path("bundles/alpha.yaml")).unwrap();
    // A FIFO would block a naive open; it must be rejected before opening.
    let status = Command::new("mkfifo")
        .arg(fixture.path("bundles/beta.yaml"))
        .status()
        .unwrap();
    assert!(status.success());
    fs::create_dir_all(fixture.path("catalog.yaml")).unwrap();
    let root = fixture.root();
    assert_eq!(
        root.load_bundle(&pid("alpha")).unwrap_err(),
        PersonaError::NotRegular
    );
    assert_eq!(
        root.load_bundle(&pid("beta")).unwrap_err(),
        PersonaError::NotRegular
    );
    assert_eq!(root.load_catalog().unwrap_err(), PersonaError::NotRegular);

    fs::remove_dir_all(fixture.path("profiles")).unwrap();
    fixture.write("profiles", "not a directory");
    assert_eq!(root.load_profiles().unwrap_err(), PersonaError::NotRegular);
    assert_eq!(
        PersonaRoot::open(&fixture.path("profiles")).unwrap_err(),
        PersonaError::NotRegular
    );
}

#[test]
fn escapes_are_impossible_through_ids() {
    // Paths come only from validated slugs, so traversal never reaches the filesystem.
    for id in ["../outside", "..", "bundles/../x", "/etc/passwd"] {
        assert_eq!(
            kanade_voice::persona::PersonaId::parse(id),
            Err(PersonaError::UnsafeId)
        );
    }
}

#[test]
fn encoding_size_and_absence_are_checked() {
    let fixture = Fixture::new();
    let mut invalid = bundle("alpha", "Alpha").into_bytes();
    invalid.extend_from_slice(b"# \xff\xfe\n");
    fixture.write("bundles/alpha.yaml", invalid);
    fixture.write("bundles/beta.yaml", "x".repeat(256 * 1024 + 1));
    let root = fixture.root();
    assert_eq!(
        root.load_bundle(&pid("alpha")).unwrap_err(),
        PersonaError::InvalidUtf8
    );
    assert_eq!(
        root.load_bundle(&pid("beta")).unwrap_err(),
        PersonaError::TooLarge
    );
    assert_eq!(
        root.load_bundle(&pid("gamma")).unwrap_err(),
        PersonaError::Missing
    );
    assert_eq!(root.load_catalog().unwrap_err(), PersonaError::Missing);
}

#[test]
fn profiles_are_whole_or_unreadable_and_example_is_never_a_candidate() {
    let fixture = Fixture::new();
    fixture.write("profiles/example.yaml", profile("example", None));
    fixture.write("profiles/good.yaml", profile("good", Some("good generic")));
    fixture.write(
        "profiles/bad.yaml",
        profile("bad", Some("x")).replace("  generic: x", "  generic: x\n  unknown: y"),
    );
    fixture.write("profiles/Upper.yaml", profile("upper", None));
    fixture.write("profiles/notes.md", "ignored");
    fixture.write("profiles/wrong.yaml", profile("other", None));
    let set = fixture.root().load_profiles().unwrap();
    assert_eq!(set.readable.keys().collect::<Vec<_>>(), [&prof("good")]);
    let mut unreadable = set
        .unreadable
        .iter()
        .map(|issue| issue.basename.as_str())
        .collect::<Vec<_>>();
    unreadable.sort_unstable();
    assert_eq!(unreadable, ["Upper.yaml", "bad.yaml", "wrong.yaml"]);
    assert!(matches!(
        fixture.root().load_profile(&prof("example")).unwrap_err(),
        PersonaError::Invalid(_)
    ));
}

#[test]
fn missing_profiles_directory_means_no_profiles() {
    let fixture = Fixture::new();
    fs::remove_dir_all(fixture.path("profiles")).unwrap();
    let set = fixture.root().load_profiles().unwrap();
    assert!(set.readable.is_empty() && set.unreadable.is_empty());
}

#[test]
fn hard_linked_files_are_rejected() {
    let fixture = Fixture::new();
    let outside = fixture.outside();
    fs::write(outside.join("Alpha.yaml"), bundle("alpha", "Alpha")).unwrap();
    fs::hard_link(
        outside.join("Alpha.yaml"),
        fixture.path("bundles/alpha.yaml"),
    )
    .unwrap();
    fs::write(outside.join("Style.yaml"), profile("style", None)).unwrap();
    fs::hard_link(
        outside.join("Style.yaml"),
        fixture.path("profiles/style.yaml"),
    )
    .unwrap();
    let root = fixture.root();
    assert_eq!(
        root.load_bundle(&pid("alpha")).unwrap_err(),
        PersonaError::Linked
    );
    let set = root.load_profiles().unwrap();
    assert!(set.readable.is_empty());
    assert_eq!(set.unreadable[0].error, PersonaError::Linked);

    // Dropping the other name leaves one link, and the file loads.
    fs::remove_file(outside.join("Alpha.yaml")).unwrap();
    root.load_bundle(&pid("alpha")).unwrap();
}

#[test]
fn more_profile_candidates_than_the_cap_fail_the_whole_load() {
    let fixture = Fixture::new();
    // Only `<stem>.yaml` other than the example counts toward the cap.
    fixture.write("profiles/example.yaml", profile("example", None));
    fixture.write("profiles/Notes.md", "ignored");
    fixture.write("profiles/good.yaml", profile("good", None));
    for index in 1..MAX_PROFILE_FILES {
        fixture.write(&format!("profiles/Filler{index:03}.yaml"), "");
    }
    let set = fixture.root().load_profiles().unwrap();
    assert_eq!(set.readable.keys().collect::<Vec<_>>(), [&prof("good")]);
    assert_eq!(set.unreadable.len(), MAX_PROFILE_FILES - 1);

    fixture.write("profiles/Filler999.yaml", "");
    assert_eq!(
        fixture.root().load_profiles().unwrap_err(),
        PersonaError::TooManyProfiles
    );
}

#[test]
fn names_differing_only_in_case_are_missing_on_every_filesystem() {
    // macOS volumes are case-insensitive and Linux (production) is not: a file
    // or directory matching the ID-derived name only up to case must be
    // missing on both, or a macOS run accepts what the deployment refuses.
    let fixture = Fixture::new();
    fixture.write("Catalog.yaml", catalog("alpha", &[("alpha", &[])]));
    fixture.write("bundles/Alpha.yaml", bundle("alpha", "Alpha"));
    fixture.write("profiles/Style.yaml", profile("style", None));
    let root = fixture.root();
    assert_eq!(root.load_catalog().unwrap_err(), PersonaError::Missing);
    assert_eq!(
        root.load_bundle(&pid("alpha")).unwrap_err(),
        PersonaError::Missing
    );
    assert_eq!(
        root.load_profile(&prof("style")).unwrap_err(),
        PersonaError::Missing
    );
    let set = root.load_profiles().unwrap();
    assert!(set.readable.is_empty());
    assert_eq!(set.unreadable[0].basename, "Style.yaml");
    assert_eq!(set.unreadable[0].error, PersonaError::UnsafeId);

    fs::rename(fixture.path("bundles"), fixture.path("Bundles")).unwrap();
    fixture.write("Bundles/beta.yaml", bundle("beta", "Beta"));
    assert_eq!(
        root.load_bundle(&pid("beta")).unwrap_err(),
        PersonaError::Missing
    );
    fs::rename(fixture.path("profiles"), fixture.path("Profiles")).unwrap();
    fixture.write("Profiles/good.yaml", profile("good", None));
    let set = root.load_profiles().unwrap();
    assert!(set.readable.is_empty() && set.unreadable.is_empty());
    assert_eq!(
        root.load_profile(&prof("good")).unwrap_err(),
        PersonaError::Missing
    );
}
