//! Strict file schemas. Text is kept exactly as parsed, including block-scalar newlines.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_saphyr::{DuplicateKeyPolicy, MergeKeyPolicy};

use super::{
    PersonaError, YamlIssue,
    id::{PersonaId, ProfileId, validate_alias},
    nudges::{Nudges, RawNudges},
    staging,
};

const SCHEMA_VERSION: u32 = 1;
const MAX_PERSONAS: usize = 100;
const MAX_ALIASES: usize = 20;
const MAX_LABEL_CHARS: usize = 200;

fn from_yaml<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, PersonaError> {
    let options = serde_saphyr::options! {
        budget: serde_saphyr::budget! {
            max_documents: 1,
            max_depth: 8,
            max_anchors: 0,
            max_aliases: 0,
            max_merge_keys: 0,
        },
        duplicate_keys: DuplicateKeyPolicy::Error,
        merge_keys: MergeKeyPolicy::Error,
        // Text fields must be strings: unquoted numbers and booleans are rejected, not coerced.
        no_schema: true,
        reject_unsupported_tags: true,
        strict_booleans: true,
        with_snippet: false,
    };
    serde_saphyr::from_str_with_options(text, options)
        .map_err(|error| PersonaError::Yaml(YamlIssue::classify(&error.to_string())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalog {
    schema_version: u32,
    default: String,
    personas: Vec<RawCatalogEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCatalogEntry {
    id: String,
    label: String,
    aliases: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBundle {
    schema_version: u32,
    id: String,
    identity: String,
    behaviour: RawBehaviour,
    staging: RawStaging,
    #[serde(default)]
    compact: Option<RawCompact>,
    #[serde(default)]
    nudges: Option<RawNudges>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBehaviour {
    #[serde(default)]
    voice: Option<String>,
    prompt: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCompact {
    #[serde(default)]
    header_rewrite: Option<String>,
    #[serde(default)]
    nudge_rewrite: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStaging {
    schedule: String,
    guide: String,
    guide_named: String,
    write: String,
    generic: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStagingOverride {
    #[serde(default)]
    schedule: Option<String>,
    #[serde(default)]
    guide: Option<String>,
    #[serde(default)]
    guide_named: Option<String>,
    #[serde(default)]
    write: Option<String>,
    #[serde(default)]
    generic: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    schema_version: u32,
    id: String,
    label: String,
    #[serde(default)]
    voice: Option<String>,
    prompt: String,
    #[serde(default)]
    staging: Option<RawStagingOverride>,
    #[serde(default)]
    nudges: Option<RawNudges>,
}

fn check_version(version: u32) -> Result<(), PersonaError> {
    if version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(PersonaError::Invalid("unsupported schema_version"))
    }
}

fn non_empty(value: String, what: &'static str) -> Result<String, PersonaError> {
    if value.trim().is_empty() {
        Err(PersonaError::Invalid(what))
    } else {
        Ok(value)
    }
}

fn one_line(value: String, what: &'static str) -> Result<String, PersonaError> {
    let value = non_empty(value, what)?;
    if value.contains(['\n', '\r']) {
        Err(PersonaError::Invalid(what))
    } else {
        Ok(value)
    }
}

fn label(value: String) -> Result<String, PersonaError> {
    let value = one_line(value, "label must be one non-empty line")?;
    if value.chars().count() > MAX_LABEL_CHARS {
        return Err(PersonaError::Invalid("label is too long"));
    }
    Ok(value)
}

fn voice(value: Option<String>) -> Result<Option<String>, PersonaError> {
    value
        .map(|value| one_line(value, "voice must be one non-empty line"))
        .transpose()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub id: PersonaId,
    pub label: String,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    default: PersonaId,
    entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn default_id(&self) -> &PersonaId {
        &self.default
    }

    pub fn entries(&self) -> &[CatalogEntry] {
        &self.entries
    }

    pub fn entry(&self, id: &PersonaId) -> Option<&CatalogEntry> {
        self.entries.iter().find(|entry| &entry.id == id)
    }

    /// Import/config boundary only: map an ID or alias to its canonical ID.
    pub fn canonicalize(&self, token: &str) -> Option<&PersonaId> {
        self.entries
            .iter()
            .find(|entry| entry.id.as_str() == token || entry.aliases.iter().any(|a| a == token))
            .map(|entry| &entry.id)
    }
}

pub fn parse_catalog(text: &str) -> Result<Catalog, PersonaError> {
    let raw: RawCatalog = from_yaml(text)?;
    check_version(raw.schema_version)?;
    if raw.personas.is_empty() || raw.personas.len() > MAX_PERSONAS {
        return Err(PersonaError::Invalid(
            "personas must be a non-empty bounded list",
        ));
    }
    let mut tokens = BTreeSet::new();
    let mut entries = Vec::with_capacity(raw.personas.len());
    for entry in raw.personas {
        let id = PersonaId::parse(&entry.id)?;
        if !tokens.insert(entry.id) {
            return Err(PersonaError::Invalid("duplicate persona ID or alias"));
        }
        if entry.aliases.len() > MAX_ALIASES {
            return Err(PersonaError::Invalid("too many aliases"));
        }
        for alias in &entry.aliases {
            validate_alias(alias)?;
        }
        entries.push(CatalogEntry {
            id,
            label: label(entry.label)?,
            aliases: entry.aliases,
        });
    }
    // Aliases may not collide with any ID, including later ones, or with each other.
    for alias in entries.iter().flat_map(|entry| &entry.aliases) {
        if !tokens.insert(alias.clone()) {
            return Err(PersonaError::Invalid("alias collides with an ID or alias"));
        }
    }
    let default = PersonaId::parse(&raw.default)?;
    if !entries.iter().any(|entry| entry.id == default) {
        return Err(PersonaError::Invalid("default must name a catalog ID"));
    }
    Ok(Catalog { default, entries })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staging {
    pub schedule: String,
    pub guide: String,
    pub guide_named: String,
    pub write: String,
    pub generic: String,
}

/// Partial profile staging; unset keys inherit the selected bundle's lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StagingOverride {
    pub schedule: Option<String>,
    pub guide: Option<String>,
    pub guide_named: Option<String>,
    pub write: Option<String>,
    pub generic: Option<String>,
}

impl StagingOverride {
    pub fn apply(&self, base: &Staging) -> Staging {
        let pick =
            |own: &Option<String>, inherited: &String| own.as_ref().unwrap_or(inherited).clone();
        Staging {
            schedule: pick(&self.schedule, &base.schedule),
            guide: pick(&self.guide, &base.guide),
            guide_named: pick(&self.guide_named, &base.guide_named),
            write: pick(&self.write, &base.write),
            generic: pick(&self.generic, &base.generic),
        }
    }
}

const STAGING_EMPTY: &str = "staging lines must be non-empty";

impl TryFrom<RawStaging> for Staging {
    type Error = PersonaError;

    fn try_from(raw: RawStaging) -> Result<Self, Self::Error> {
        let parsed = Self {
            schedule: non_empty(raw.schedule, STAGING_EMPTY)?,
            guide: non_empty(raw.guide, STAGING_EMPTY)?,
            guide_named: non_empty(raw.guide_named, STAGING_EMPTY)?,
            write: non_empty(raw.write, STAGING_EMPTY)?,
            generic: non_empty(raw.generic, STAGING_EMPTY)?,
        };
        staging::validate(&parsed)?;
        Ok(parsed)
    }
}

impl TryFrom<RawStagingOverride> for StagingOverride {
    type Error = PersonaError;

    fn try_from(raw: RawStagingOverride) -> Result<Self, Self::Error> {
        let line = |value: Option<String>| value.map(|v| non_empty(v, STAGING_EMPTY)).transpose();
        let parsed = Self {
            schedule: line(raw.schedule)?,
            guide: line(raw.guide)?,
            guide_named: line(raw.guide_named)?,
            write: line(raw.write)?,
            generic: line(raw.generic)?,
        };
        staging::validate_override(&parsed)?;
        Ok(parsed)
    }
}

/// One-line rewrite instructions for a small model; v5-only, no v4 oracle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Compact {
    /// Reminder header-line rewrite.
    pub header_rewrite: Option<String>,
    /// Nudge lead-in rewrite; unset means the caller summarises identity and voice.
    pub nudge_rewrite: Option<String>,
}

impl TryFrom<RawCompact> for Compact {
    type Error = PersonaError;

    fn try_from(raw: RawCompact) -> Result<Self, Self::Error> {
        let text = |value: Option<String>, what| value.map(|v| non_empty(v, what)).transpose();
        let compact = Self {
            header_rewrite: text(raw.header_rewrite, "header_rewrite must be non-empty")?,
            nudge_rewrite: text(raw.nudge_rewrite, "nudge_rewrite must be non-empty")?,
        };
        if compact.header_rewrite.is_none() && compact.nudge_rewrite.is_none() {
            return Err(PersonaError::Invalid(
                "compact must declare a rewrite prompt",
            ));
        }
        Ok(compact)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub id: PersonaId,
    pub identity: String,
    pub voice: Option<String>,
    pub prompt: String,
    pub staging: Staging,
    pub compact: Option<Compact>,
    pub nudges: Option<Nudges>,
}

/// Parse a bundle whose file name was derived from `expected`.
pub fn parse_bundle(text: &str, expected: &PersonaId) -> Result<Bundle, PersonaError> {
    let raw: RawBundle = from_yaml(text)?;
    check_version(raw.schema_version)?;
    if PersonaId::parse(&raw.id)? != *expected {
        return Err(PersonaError::Invalid(
            "bundle id does not match its file name",
        ));
    }
    Ok(Bundle {
        id: expected.clone(),
        identity: non_empty(raw.identity, "identity must be non-empty")?,
        voice: voice(raw.behaviour.voice)?,
        prompt: non_empty(raw.behaviour.prompt, "behaviour prompt must be non-empty")?,
        staging: raw.staging.try_into()?,
        compact: raw.compact.map(Compact::try_from).transpose()?,
        nudges: raw.nudges.map(Nudges::try_from).transpose()?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub id: ProfileId,
    pub label: String,
    pub voice: Option<String>,
    pub prompt: String,
    pub staging: StagingOverride,
    /// Seed pools tried before the bundle's.
    pub nudges: Option<Nudges>,
}

/// Parse a profile whose file name was derived from `expected`.
pub fn parse_profile(text: &str, expected: &ProfileId) -> Result<Profile, PersonaError> {
    let raw: RawProfile = from_yaml(text)?;
    check_version(raw.schema_version)?;
    if ProfileId::parse(&raw.id)? != *expected {
        return Err(PersonaError::Invalid(
            "profile id does not match its file name",
        ));
    }
    Ok(Profile {
        id: expected.clone(),
        label: label(raw.label)?,
        voice: voice(raw.voice)?,
        prompt: non_empty(raw.prompt, "profile prompt must be non-empty")?,
        staging: raw.staging.unwrap_or_default().try_into()?,
        nudges: raw.nudges.map(Nudges::try_from).transpose()?,
    })
}
