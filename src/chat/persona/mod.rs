//! v5 persona catalog, bundles and reply profiles under `config/personas/`.

mod error;
mod id;
mod loader;
mod resolver;
mod schema;
mod snapshot;

pub use error::{PersonaError, YamlIssue};
pub use id::{EXAMPLE_PROFILE, FALLBACK_PERSONA, PersonaId, ProfileId, RoleId};
pub use loader::{Loaded, PersonaRoot, ProfileIssue, ProfileSet, Source};
pub use resolver::{
    CandidateIssue, ProfileQuery, ProfileSource, RoleAssignment, SelectionSource, resolve_profile,
};
pub use schema::{
    Bundle, Catalog, CatalogEntry, Compact, Profile, Staging, StagingOverride, parse_bundle,
    parse_catalog, parse_profile,
};
pub use snapshot::{
    ActivePersona, PersonaSnapshot, PersonaStore, Provenance, ReloadError, ReloadOutcome,
    ResolvedPersona,
};
