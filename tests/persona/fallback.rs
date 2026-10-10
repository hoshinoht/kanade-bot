use std::{
    collections::BTreeSet,
    os::unix::fs::symlink,
    sync::{Arc, Barrier},
};

use kanade::chat::persona::{
    PersonaError, PersonaSnapshot, PersonaStore, ProfileQuery, ReloadError, SelectionSource,
};

use crate::support::{Fixture, bundle, catalog, pid, profile};

fn effective(snapshot: &PersonaSnapshot) -> Option<(String, SelectionSource)> {
    let provenance = snapshot.provenance();
    Some((
        provenance.effective.as_ref()?.to_string(),
        provenance.source?,
    ))
}

fn with_catalog() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        "catalog.yaml",
        catalog(
            "alpha",
            &[("alpha", &[]), ("beta", &["sakuna"]), ("kanade", &[])],
        ),
    );
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    fixture.write("bundles/beta.yaml", bundle("beta", "Beta"));
    fixture
}

const BROKEN: &str = "schema_version: 1\nid: broken\n";

#[test]
fn startup_fallback_matrix() {
    use SelectionSource::*;
    let startup = |fixture: &Fixture, configured: Option<&str>| {
        PersonaSnapshot::startup(&fixture.root(), configured.map(pid).as_ref())
    };

    let fixture = with_catalog();
    assert_eq!(
        effective(&startup(&fixture, Some("beta"))),
        Some(("beta".into(), Configured))
    );
    assert_eq!(
        effective(&startup(&fixture, None)),
        Some(("alpha".into(), CatalogDefault))
    );
    // Unknown IDs and aliases are not resolved at runtime.
    for configured in ["gamma", "sakuna"] {
        let snapshot = startup(&fixture, Some(configured));
        assert_eq!(effective(&snapshot), Some(("alpha".into(), CatalogDefault)));
        assert_eq!(snapshot.provenance().configured, Some(pid(configured)));
        assert_eq!(
            snapshot.provenance().issues[0].error,
            PersonaError::NotInCatalog
        );
    }

    fixture.write("bundles/beta.yaml", BROKEN);
    assert_eq!(
        effective(&startup(&fixture, Some("beta"))),
        Some(("alpha".into(), CatalogDefault))
    );

    fixture.write("bundles/alpha.yaml", BROKEN);
    let snapshot = startup(&fixture, Some("beta"));
    assert_eq!(
        effective(&snapshot),
        Some(("kanade".into(), TrackedFallback))
    );
    let candidates = snapshot
        .provenance()
        .issues
        .iter()
        .map(|issue| issue.candidate)
        .collect::<Vec<_>>();
    assert_eq!(candidates, [Some(Configured), Some(CatalogDefault)]);

    // Configured equals default: it is tried once, then the tracked fallback.
    assert_eq!(
        effective(&startup(&fixture, Some("alpha"))),
        Some(("kanade".into(), TrackedFallback))
    );
    // Kanade listed in the catalog and selected directly is a configured selection.
    assert_eq!(
        effective(&startup(&fixture, Some("kanade"))),
        Some(("kanade".into(), Configured))
    );

    fixture.write("catalog.yaml", "schema_version: 1\nschema_version: 1\n");
    let snapshot = startup(&fixture, Some("beta"));
    assert_eq!(
        effective(&snapshot),
        Some(("kanade".into(), TrackedFallback))
    );
    assert_eq!(snapshot.provenance().issues[0].candidate, None);

    fixture.remove("catalog.yaml");
    assert_eq!(
        effective(&startup(&fixture, Some("beta"))),
        Some(("kanade".into(), TrackedFallback))
    );
}

#[test]
fn catalog_kanade_not_listed_still_allows_tracked_fallback() {
    let fixture = Fixture::new();
    fixture.write("catalog.yaml", catalog("alpha", &[("alpha", &[])]));
    fixture.write("bundles/alpha.yaml", BROKEN);
    let snapshot = PersonaSnapshot::startup(&fixture.root(), Some(&pid("kanade")));
    assert_eq!(
        effective(&snapshot),
        Some(("kanade".into(), SelectionSource::TrackedFallback))
    );
}

#[test]
fn chat_is_disabled_only_when_no_trusted_candidate_validates() {
    let query = ProfileQuery {
        member_roles: &[],
        role_assignments: &[],
        saved_selection: None,
        selectable: &BTreeSet::new(),
    };
    let fixture = with_catalog();
    fixture.write("bundles/alpha.yaml", BROKEN);
    fixture.write("bundles/beta.yaml", BROKEN);
    fixture.write("bundles/kanade.yaml", BROKEN);
    let snapshot = PersonaSnapshot::startup(&fixture.root(), Some(&pid("beta")));
    assert!(snapshot.active().is_none());
    assert!(snapshot.resolve(&query).is_none());
    assert_eq!(snapshot.provenance().effective, None);
    // Configured, catalog default, and tracked fallback each failed once.
    assert_eq!(snapshot.provenance().issues.len(), 3);

    let fixture = Fixture::new();
    fixture.remove("bundles/kanade.yaml");
    assert!(
        PersonaSnapshot::startup(&fixture.root(), None)
            .active()
            .is_none()
    );

    let fixture = Fixture::new();
    let outside = fixture.outside().join("kanade.yaml");
    std::fs::rename(fixture.path("bundles/kanade.yaml"), &outside).unwrap();
    symlink(&outside, fixture.path("bundles/kanade.yaml")).unwrap();
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    assert!(snapshot.active().is_none());
    assert_eq!(snapshot.provenance().issues[1].error, PersonaError::Symlink);
}

#[test]
fn legacy_layout_is_never_probed() {
    let fixture = Fixture::empty();
    fixture.write("personas.yaml", catalog("kanade", &[("kanade", &[])]));
    fixture.write("personas/kanade/identity.md", "# Persona: Legacy\n");
    fixture.write("personas/kanade/default.md", "Legacy behaviour.\n");
    fixture.write("personas/kanade/staging.yaml", "generic: legacy\n");
    fixture.write("persona.md", "# Persona: Legacy\n");
    fixture.write("behaviours/default.md", "Legacy behaviour.\n");
    let snapshot = PersonaSnapshot::startup(&fixture.root(), Some(&pid("kanade")));
    assert!(snapshot.active().is_none());
    let errors = snapshot
        .provenance()
        .issues
        .iter()
        .map(|issue| issue.error.clone())
        .collect::<Vec<_>>();
    assert_eq!(errors, [PersonaError::Missing, PersonaError::Missing]);
}

#[test]
fn failed_reload_keeps_saved_selection_and_last_known_good() {
    let fixture = with_catalog();
    let root = fixture.root();
    let store = PersonaStore::new(PersonaSnapshot::startup(&root, Some(&pid("alpha"))));
    let before = store.pin();
    let mut saved = "alpha".to_owned();

    fixture.write("bundles/beta.yaml", BROKEN);
    for requested in ["beta", "gamma", "kanade-missing"] {
        let result = store.reload(&root, &pid(requested), |id| {
            saved = id.to_string();
            Ok::<_, ()>(())
        });
        assert!(matches!(result, Err(ReloadError::Invalid(_))));
    }
    // An explicit reload never falls back, even to the tracked bundle.
    fixture.write("catalog.yaml", "not: [valid");
    let result = store.reload(&root, &pid("alpha"), |_| Ok::<_, ()>(()));
    assert!(matches!(
        result,
        Err(ReloadError::Invalid(PersonaError::Yaml(_)))
    ));
    assert_eq!(saved, "alpha");
    assert!(Arc::ptr_eq(&before, &store.pin()));

    fixture.write(
        "catalog.yaml",
        catalog("alpha", &[("alpha", &[]), ("beta", &[])]),
    );
    fixture.write("bundles/beta.yaml", bundle("beta", "Beta"));
    let result = store.reload(&root, &pid("beta"), |_| Err("disk full"));
    assert!(matches!(result, Err(ReloadError::Persist("disk full"))));
    assert!(Arc::ptr_eq(&before, &store.pin()));

    let outcome = store
        .reload(&root, &pid("beta"), |id| {
            saved = id.to_string();
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(outcome.identity_changed);
    assert_eq!(saved, "beta");
    assert_eq!(
        effective(&store.pin()),
        Some(("beta".into(), SelectionSource::Configured))
    );
}

#[test]
fn reload_with_an_unusable_profile_directory_keeps_last_known_good() {
    let fixture = with_catalog();
    let root = fixture.root();
    fixture.write("profiles/style.yaml", profile("style", Some("kept")));
    let store = PersonaStore::new(PersonaSnapshot::startup(&root, Some(&pid("alpha"))));
    let before = store.pin();

    std::fs::remove_dir_all(fixture.path("profiles")).unwrap();
    symlink(fixture.outside(), fixture.path("profiles")).unwrap();
    let result = store.reload(&root, &pid("alpha"), |_| Ok::<_, ()>(()));
    assert!(matches!(result, Err(ReloadError::Invalid(_))));
    assert!(Arc::ptr_eq(&before, &store.pin()));

    // A missing directory is a valid "no profiles" layout, not a failure.
    std::fs::remove_file(fixture.path("profiles")).unwrap();
    assert!(
        store
            .reload(&root, &pid("alpha"), |_| Ok::<_, ()>(()))
            .is_ok()
    );
}

#[test]
fn identity_changes_signal_history_clearing_but_profile_changes_do_not() {
    let fixture = with_catalog();
    let root = fixture.root();
    let store = PersonaStore::new(PersonaSnapshot::startup(&root, Some(&pid("alpha"))));
    let ok = |_: &kanade::chat::persona::PersonaId| Ok::<_, ()>(());

    fixture.write("profiles/style.yaml", profile("style", Some("new")));
    assert!(
        !store
            .reload(&root, &pid("alpha"), ok)
            .unwrap()
            .identity_changed
    );
    fixture.write(
        "bundles/alpha.yaml",
        bundle("alpha", "Alpha").replace("Synthetic behaviour", "Edited behaviour"),
    );
    assert!(
        !store
            .reload(&root, &pid("alpha"), ok)
            .unwrap()
            .identity_changed
    );
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Renamed"));
    assert!(
        store
            .reload(&root, &pid("alpha"), ok)
            .unwrap()
            .identity_changed
    );
    assert!(
        store
            .reload(&root, &pid("beta"), ok)
            .unwrap()
            .identity_changed
    );

    // Recovering from a disabled startup is an identity change.
    let empty = Fixture::new();
    empty.remove("bundles/kanade.yaml");
    let disabled = PersonaStore::new(PersonaSnapshot::startup(&empty.root(), None));
    empty.write("catalog.yaml", catalog("alpha", &[("alpha", &[])]));
    empty.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    assert!(
        disabled
            .reload(&empty.root(), &pid("alpha"), ok)
            .unwrap()
            .identity_changed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pinned_turn_keeps_its_snapshot_across_a_swap() {
    let fixture = with_catalog();
    let root = fixture.root();
    let store = Arc::new(PersonaStore::new(PersonaSnapshot::startup(
        &root,
        Some(&pid("alpha")),
    )));
    let pinned = Arc::new(Barrier::new(2));
    let swapped = Arc::new(Barrier::new(2));

    let turn = tokio::spawn({
        let store = Arc::clone(&store);
        let (pinned, swapped) = (Arc::clone(&pinned), Arc::clone(&swapped));
        async move {
            let snapshot = store.pin();
            pinned.wait();
            swapped.wait();
            tokio::task::yield_now().await;
            let identity = snapshot.active().unwrap().bundle.value.identity.clone();
            (
                identity,
                store.pin().active().unwrap().bundle.value.id.to_string(),
            )
        }
    });
    tokio::task::spawn_blocking({
        let (store, root) = (Arc::clone(&store), root.clone());
        move || {
            pinned.wait();
            store
                .reload(&root, &pid("beta"), |_| Ok::<_, ()>(()))
                .unwrap();
            swapped.wait();
        }
    })
    .await
    .unwrap();
    let (identity, later) = turn.await.unwrap();
    assert!(identity.contains("Alpha"));
    assert_eq!(later, "beta");
}
