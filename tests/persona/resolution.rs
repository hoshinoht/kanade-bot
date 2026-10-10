use std::collections::BTreeSet;

use kanade::chat::persona::{
    PersonaSnapshot, PersonaStore, ProfileId, ProfileQuery, ProfileSource, RoleAssignment, RoleId,
};

use crate::support::{Fixture, bundle, catalog, pid, prof, profile};

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write("catalog.yaml", catalog("alpha", &[("alpha", &[])]));
    fixture.write("bundles/alpha.yaml", bundle("alpha", "Alpha"));
    fixture.write(
        "profiles/first.yaml",
        profile("first", Some("first generic")),
    );
    fixture.write("profiles/second.yaml", profile("second", None));
    fixture.write("profiles/broken.yaml", "schema_version: 1\nid: broken\n");
    fixture
}

fn assignments() -> Vec<RoleAssignment> {
    [
        ("role-broken", "broken"),
        ("role-first", "first"),
        ("role-second", "second"),
    ]
    .into_iter()
    .map(|(role, profile)| RoleAssignment {
        role: RoleId::new(role),
        profile: prof(profile),
    })
    .collect()
}

fn resolved(
    snapshot: &PersonaSnapshot,
    roles: &[&str],
    saved: Option<&ProfileId>,
    selectable: &BTreeSet<ProfileId>,
) -> (Option<String>, ProfileSource) {
    let roles = roles
        .iter()
        .map(|role| RoleId::new(*role))
        .collect::<Vec<_>>();
    let assignments = assignments();
    let query = ProfileQuery {
        member_roles: &roles,
        role_assignments: &assignments,
        saved_selection: saved,
        selectable,
    };
    let resolved = snapshot.resolve(&query).expect("chat is enabled");
    (
        resolved.profile.map(|profile| profile.id.to_string()),
        resolved.profile_source,
    )
}

#[test]
fn first_readable_role_assignment_wins_over_saved_selection() {
    let fixture = fixture();
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    let selectable = BTreeSet::from([prof("first"), prof("second")]);
    let saved = prof("second");
    // The unreadable first assignment is skipped, not partially applied.
    assert_eq!(
        resolved(
            &snapshot,
            &["role-second", "role-broken", "role-first"],
            Some(&saved),
            &selectable
        ),
        (Some("first".into()), ProfileSource::RoleAssignment)
    );
    assert_eq!(
        resolved(&snapshot, &["role-broken"], Some(&saved), &selectable),
        (Some("second".into()), ProfileSource::MemberSelection)
    );
    // Role profiles need not be member-selectable.
    assert_eq!(
        resolved(&snapshot, &["role-first"], None, &BTreeSet::new()),
        (Some("first".into()), ProfileSource::RoleAssignment)
    );
    assert_eq!(
        resolved(&snapshot, &["unassigned"], None, &selectable),
        (
            None,
            ProfileSource::BundleDefault {
                saved_selection_unavailable: false
            }
        )
    );
}

#[test]
fn unavailable_saved_selection_falls_back_without_being_cleared() {
    let fixture = fixture();
    let root = fixture.root();
    let store = PersonaStore::new(PersonaSnapshot::startup(&root, Some(&pid("alpha"))));
    let saved = prof("later");
    let selectable = BTreeSet::from([prof("later"), prof("second")]);
    let unavailable = (
        None,
        ProfileSource::BundleDefault {
            saved_selection_unavailable: true,
        },
    );
    assert_eq!(
        resolved(&store.pin(), &[], Some(&saved), &selectable),
        unavailable
    );
    // Unpublished (not selectable) readable profiles are also unavailable to members.
    let second = prof("second");
    assert_eq!(
        resolved(&store.pin(), &[], Some(&second), &BTreeSet::new()),
        unavailable
    );
    // The example template is never a candidate even when saved and "selectable".
    let example = prof("example");
    fixture.write("profiles/example.yaml", profile("example", None));
    store
        .reload(&root, &pid("alpha"), |_| Ok::<_, ()>(()))
        .unwrap();
    assert_eq!(
        resolved(
            &store.pin(),
            &[],
            Some(&example),
            &BTreeSet::from([example.clone()])
        ),
        unavailable
    );

    fixture.write("profiles/later.yaml", profile("later", None));
    let outcome = store
        .reload(&root, &pid("alpha"), |_| Ok::<_, ()>(()))
        .unwrap();
    assert!(!outcome.identity_changed);
    assert_eq!(
        resolved(&store.pin(), &[], Some(&saved), &selectable),
        (Some("later".into()), ProfileSource::MemberSelection)
    );
}

#[test]
fn profile_staging_inherits_missing_lines_from_the_bundle() {
    let fixture = fixture();
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    let roles = [RoleId::new("role-first")];
    let assignments = assignments();
    let selectable = BTreeSet::new();
    let query = ProfileQuery {
        member_roles: &roles,
        role_assignments: &assignments,
        saved_selection: None,
        selectable: &selectable,
    };
    let resolved = snapshot.resolve(&query).unwrap();
    assert_eq!(resolved.staging.generic, "first generic");
    assert_eq!(resolved.staging.schedule, "alpha schedule");
    assert_eq!(resolved.staging.guide_named, "{boss} alpha guide");
    assert_eq!(resolved.profile_file.unwrap().basename, "first.yaml");

    let none = ProfileQuery {
        member_roles: &[],
        ..query
    };
    assert_eq!(
        snapshot.resolve(&none).unwrap().staging,
        resolved.bundle.staging
    );
}

#[test]
fn role_ids_never_reach_debug_output() {
    let fixture = fixture();
    let snapshot = PersonaSnapshot::startup(&fixture.root(), None);
    let roles = [RoleId::new("123456789012345678")];
    let assignments = [RoleAssignment {
        role: RoleId::new("123456789012345678"),
        profile: prof("first"),
    }];
    let selectable = BTreeSet::new();
    let query = ProfileQuery {
        member_roles: &roles,
        role_assignments: &assignments,
        saved_selection: None,
        selectable: &selectable,
    };
    let rendered = format!("{:?} {:?}", query, snapshot.resolve(&query).unwrap());
    assert!(!rendered.contains("123456789012345678"));
    assert!(!format!("{:?}", snapshot).contains(&*fixture.dir.to_string_lossy()));
}
