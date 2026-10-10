//! Bundle candidate order and reply-profile precedence.

use std::collections::BTreeSet;

use super::{
    PersonaError,
    id::{FALLBACK_PERSONA, PersonaId, ProfileId, RoleId},
    loader::{Loaded, PersonaRoot, ProfileSet},
    schema::{Bundle, Catalog, Profile},
};

/// Which candidate supplied the effective bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionSource {
    Configured,
    CatalogDefault,
    TrackedFallback,
}

/// Why a candidate (or the catalog, when `candidate` is `None`) was skipped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateIssue {
    pub candidate: Option<SelectionSource>,
    pub error: PersonaError,
}

pub(crate) struct Selected {
    pub catalog: Option<Loaded<Catalog>>,
    pub bundle: Option<(Loaded<Bundle>, SelectionSource)>,
    pub issues: Vec<CandidateIssue>,
}

fn catalog_bundle(
    root: &PersonaRoot,
    catalog: &Catalog,
    id: &PersonaId,
) -> Result<Loaded<Bundle>, PersonaError> {
    if catalog.entry(id).is_none() {
        return Err(PersonaError::NotInCatalog);
    }
    root.load_bundle(id)
}

/// Startup order: configured bundle, catalog default, tracked Kanade, else none.
pub(crate) fn select_startup(root: &PersonaRoot, configured: Option<&PersonaId>) -> Selected {
    let mut issues = Vec::new();
    let mut seen = BTreeSet::new();
    // Bundle files that already failed; a catalog-membership miss does not count.
    let mut failed_files = BTreeSet::new();
    let catalog = root
        .load_catalog()
        .map_err(|error| {
            issues.push(CandidateIssue {
                candidate: None,
                error,
            })
        })
        .ok();
    if let Some(catalog) = &catalog {
        let candidates = [
            (configured, SelectionSource::Configured),
            (
                Some(catalog.value.default_id()),
                SelectionSource::CatalogDefault,
            ),
        ];
        for (id, source) in candidates {
            let Some(id) = id else { continue };
            if !seen.insert(id.clone()) {
                continue;
            }
            match catalog_bundle(root, &catalog.value, id) {
                Ok(bundle) => {
                    return Selected {
                        catalog: Some(catalog.clone()),
                        bundle: Some((bundle, source)),
                        issues,
                    };
                }
                Err(error) => {
                    if error != PersonaError::NotInCatalog {
                        failed_files.insert(id.clone());
                    }
                    issues.push(CandidateIssue {
                        candidate: Some(source),
                        error,
                    });
                }
            }
        }
    }
    let fallback = PersonaId::parse(FALLBACK_PERSONA).expect("fallback ID is a valid slug");
    let bundle = if failed_files.contains(&fallback) {
        None
    } else {
        root.load_bundle(&fallback)
            .map_err(|error| {
                issues.push(CandidateIssue {
                    candidate: Some(SelectionSource::TrackedFallback),
                    error,
                })
            })
            .ok()
            .map(|bundle| (bundle, SelectionSource::TrackedFallback))
    };
    Selected {
        catalog,
        bundle,
        issues,
    }
}

/// Explicit switch/reload: the requested catalog bundle or an error, never a fallback.
pub(crate) fn select_explicit(
    root: &PersonaRoot,
    requested: &PersonaId,
) -> Result<(Loaded<Catalog>, Loaded<Bundle>), PersonaError> {
    let catalog = root.load_catalog()?;
    let bundle = catalog_bundle(root, &catalog.value, requested)?;
    Ok((catalog, bundle))
}

#[derive(Clone, Debug)]
pub struct RoleAssignment {
    pub role: RoleId,
    pub profile: ProfileId,
}

/// Style inputs for one member. Chat admission is decided elsewhere; nothing here grants it.
#[derive(Clone, Copy, Debug)]
pub struct ProfileQuery<'a> {
    pub member_roles: &'a [RoleId],
    /// Ordered; the first readable match wins.
    pub role_assignments: &'a [RoleAssignment],
    /// Saved selection; kept by the caller even while unavailable.
    pub saved_selection: Option<&'a ProfileId>,
    /// Profiles members may currently choose.
    pub selectable: &'a BTreeSet<ProfileId>,
}

/// Where the effective profile came from. Role IDs are deliberately not recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileSource {
    RoleAssignment,
    MemberSelection,
    BundleDefault { saved_selection_unavailable: bool },
}

pub fn resolve_profile<'s>(
    profiles: &'s ProfileSet,
    query: &ProfileQuery<'_>,
) -> (Option<&'s Loaded<Profile>>, ProfileSource) {
    let by_role = query
        .role_assignments
        .iter()
        .filter(|assignment| query.member_roles.contains(&assignment.role))
        .find_map(|assignment| profiles.get(&assignment.profile));
    if let Some(profile) = by_role {
        return (Some(profile), ProfileSource::RoleAssignment);
    }
    let Some(saved) = query.saved_selection else {
        return (
            None,
            ProfileSource::BundleDefault {
                saved_selection_unavailable: false,
            },
        );
    };
    match profiles.get(saved) {
        Some(profile) if query.selectable.contains(saved) => {
            (Some(profile), ProfileSource::MemberSelection)
        }
        _ => (
            None,
            ProfileSource::BundleDefault {
                saved_selection_unavailable: true,
            },
        ),
    }
}
