//! Immutable persona snapshots, pinned per turn and swapped atomically.

use std::sync::{Arc, Mutex, PoisonError, RwLock};

use super::{
    PersonaError,
    id::PersonaId,
    loader::{Loaded, PersonaRoot, ProfileSet, Source},
    resolver::{
        CandidateIssue, ProfileQuery, ProfileSource, SelectionSource, resolve_profile,
        select_explicit, select_startup,
    },
    schema::{Bundle, Catalog, Profile, Staging},
};

/// Diagnostics for operators; never part of model-visible text.
#[derive(Clone, Debug)]
pub struct Provenance {
    pub configured: Option<PersonaId>,
    pub effective: Option<PersonaId>,
    pub source: Option<SelectionSource>,
    pub bundle: Option<Source>,
    pub issues: Vec<CandidateIssue>,
    /// Set when the profile directory itself was unusable.
    pub profiles_issue: Option<PersonaError>,
}

#[derive(Debug)]
pub struct ActivePersona {
    pub catalog: Option<Loaded<Catalog>>,
    pub bundle: Loaded<Bundle>,
    pub profiles: ProfileSet,
}

#[derive(Debug)]
pub struct PersonaSnapshot {
    active: Option<ActivePersona>,
    provenance: Provenance,
}

/// One member's persona for one turn, borrowed from a pinned snapshot.
#[derive(Debug)]
pub struct ResolvedPersona<'a> {
    pub bundle: &'a Bundle,
    pub profile: Option<&'a Profile>,
    pub staging: Staging,
    pub profile_source: ProfileSource,
    pub bundle_source: &'a Source,
    pub profile_file: Option<&'a Source>,
}

fn profiles(root: &PersonaRoot) -> (ProfileSet, Option<PersonaError>) {
    match root.load_profiles() {
        Ok(set) => (set, None),
        Err(error) => (ProfileSet::default(), Some(error)),
    }
}

impl PersonaSnapshot {
    /// Startup selection with the trusted fallback chain; disabled if nothing validates.
    pub fn startup(root: &PersonaRoot, configured: Option<&PersonaId>) -> Self {
        let selected = select_startup(root, configured);
        let mut provenance = Provenance {
            configured: configured.cloned(),
            effective: None,
            source: None,
            bundle: None,
            issues: selected.issues,
            profiles_issue: None,
        };
        let active = selected.bundle.map(|(bundle, source)| {
            let (profiles, profiles_issue) = profiles(root);
            provenance.effective = Some(bundle.value.id.clone());
            provenance.source = Some(source);
            provenance.bundle = Some(bundle.source.clone());
            provenance.profiles_issue = profiles_issue;
            ActivePersona {
                catalog: selected.catalog,
                bundle,
                profiles,
            }
        });
        Self { active, provenance }
    }

    /// Strict selection for an explicit switch or reload; never falls back.
    /// An unusable profile directory fails the reload so last-known-good
    /// profiles are not silently dropped; a missing one means no profiles.
    pub fn explicit(root: &PersonaRoot, requested: &PersonaId) -> Result<Self, PersonaError> {
        let (catalog, bundle) = select_explicit(root, requested)?;
        let profiles = root.load_profiles()?;
        let provenance = Provenance {
            configured: Some(requested.clone()),
            effective: Some(bundle.value.id.clone()),
            source: Some(SelectionSource::Configured),
            bundle: Some(bundle.source.clone()),
            issues: Vec::new(),
            profiles_issue: None,
        };
        Ok(Self {
            active: Some(ActivePersona {
                catalog: Some(catalog),
                bundle,
                profiles,
            }),
            provenance,
        })
    }

    /// `None` means chat is disabled and no model call may be made.
    pub fn active(&self) -> Option<&ActivePersona> {
        self.active.as_ref()
    }

    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    pub fn resolve(&self, query: &ProfileQuery<'_>) -> Option<ResolvedPersona<'_>> {
        let active = self.active.as_ref()?;
        let (profile, profile_source) = resolve_profile(&active.profiles, query);
        let bundle = &active.bundle.value;
        let staging = profile.map_or_else(
            || bundle.staging.clone(),
            |profile| profile.value.staging.apply(&bundle.staging),
        );
        Some(ResolvedPersona {
            bundle,
            profile: profile.map(|profile| &profile.value),
            staging,
            profile_source,
            bundle_source: &active.bundle.source,
            profile_file: profile.map(|profile| &profile.source),
        })
    }

    fn identity_key(&self) -> Option<(&PersonaId, &str)> {
        self.active.as_ref().map(|active| {
            (
                &active.bundle.value.id,
                active.bundle.value.identity.as_str(),
            )
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReloadOutcome {
    /// History and anchors must be cleared; profile-only changes never set this.
    pub identity_changed: bool,
}

#[derive(Debug)]
pub enum ReloadError<E> {
    Invalid(PersonaError),
    Persist(E),
}

/// Holds the current snapshot. Turns pin an `Arc` before any await.
#[derive(Debug)]
pub struct PersonaStore {
    current: RwLock<Arc<PersonaSnapshot>>,
    reload_gate: Mutex<()>,
}

impl PersonaStore {
    pub fn new(snapshot: PersonaSnapshot) -> Self {
        Self {
            current: RwLock::new(Arc::new(snapshot)),
            reload_gate: Mutex::new(()),
        }
    }

    pub fn pin(&self) -> Arc<PersonaSnapshot> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Validate, persist, then swap. Any failure leaves the saved selection and
    /// the last-known-good snapshot untouched.
    pub fn reload<E>(
        &self,
        root: &PersonaRoot,
        requested: &PersonaId,
        persist: impl FnOnce(&PersonaId) -> Result<(), E>,
    ) -> Result<ReloadOutcome, ReloadError<E>> {
        let _gate = self
            .reload_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let next = PersonaSnapshot::explicit(root, requested).map_err(ReloadError::Invalid)?;
        persist(requested).map_err(ReloadError::Persist)?;
        let identity_changed = self.pin().identity_key() != next.identity_key();
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(next);
        Ok(ReloadOutcome { identity_changed })
    }
}
