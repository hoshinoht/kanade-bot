//! v5 persona catalog, bundles and reply profiles under `config/personas/`,
//! and their compilation into chat prompts.

mod compiler;
mod error;
mod id;
mod loader;
mod markdown;
mod nudges;
mod prompt;
mod resolver;
mod schema;
mod snapshot;
mod staging;

pub use compiler::{CompileProvenance, CompiledPersona, ExampleSource, NudgeSeeds, VoiceSource};
pub use error::{PersonaError, YamlIssue};
pub use id::{EXAMPLE_PROFILE, FALLBACK_PERSONA, PersonaId, ProfileId, RoleId};
pub use loader::{Loaded, PersonaRoot, ProfileIssue, ProfileSet, Source};
pub use nudges::{
    MAX_NUDGE_CHARS, MAX_NUDGE_LINES, MIN_NUDGE_LINES, MoodPools, NUDGE_FIELDS, NudgeMood,
    NudgePurpose, NudgeSource, Nudges, check_nudge_line, fill_nudge,
};
pub use prompt::TurnContext;
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
pub use staging::{BOSS_FIELD, MAX_RENDERED_STAGING_CHARS, MAX_STAGING_CHARS, StagingState};

pub(crate) use markdown::strip as py_strip;
