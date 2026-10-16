//! Contained reads of the v5 layout only: `catalog.yaml`, `bundles/`, `profiles/`.

use std::{
    collections::BTreeMap,
    fs::{self, Metadata},
    io,
    path::{Path, PathBuf},
};

use ring::digest::{SHA256, digest};

use super::{
    PersonaError,
    id::{EXAMPLE_PROFILE, PersonaId, ProfileId},
    schema::{Bundle, Catalog, Profile, parse_bundle, parse_catalog, parse_profile},
};
use crate::infrastructure::files::{Refusal, open_regular, read_capped};

const CATALOG_FILE: &str = "catalog.yaml";
const BUNDLES_DIR: &str = "bundles";
const PROFILES_DIR: &str = "profiles";
const EXTENSION: &str = ".yaml";
const MAX_FILE_BYTES: u64 = 256 * 1024;
/// Profile files a directory may offer; more fails the whole profile load
/// rather than silently dropping some.
pub const MAX_PROFILE_FILES: usize = 256;

/// Non-model-visible provenance: file basename and SHA-256 of its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub basename: String,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub source: Source,
}

#[derive(Clone, Debug)]
pub struct ProfileIssue {
    pub basename: String,
    pub error: PersonaError,
}

/// Readable profiles; invalid files are recorded whole and never partially used.
#[derive(Clone, Debug, Default)]
pub struct ProfileSet {
    pub readable: BTreeMap<ProfileId, Loaded<Profile>>,
    pub unreadable: Vec<ProfileIssue>,
}

impl ProfileSet {
    pub fn get(&self, id: &ProfileId) -> Option<&Loaded<Profile>> {
        self.readable.get(id)
    }
}

/// A validated persona directory. Its absolute path is never exposed.
#[derive(Clone)]
pub struct PersonaRoot {
    canonical: PathBuf,
}

impl std::fmt::Debug for PersonaRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PersonaRoot(<private>)")
    }
}

fn io_error(error: io::Error) -> PersonaError {
    match error.kind() {
        io::ErrorKind::NotFound => PersonaError::Missing,
        kind => PersonaError::Io(kind),
    }
}

fn refused(refusal: Refusal) -> PersonaError {
    match refusal {
        Refusal::Symlink => PersonaError::Symlink,
        Refusal::NotRegular => PersonaError::NotRegular,
        Refusal::Linked => PersonaError::Linked,
        Refusal::TooLarge => PersonaError::TooLarge,
        Refusal::Io(error) => io_error(error),
    }
}

fn check_dir(path: &Path) -> Result<(), PersonaError> {
    let metadata = fs::symlink_metadata(path).map_err(io_error)?;
    if metadata.file_type().is_symlink() {
        Err(PersonaError::Symlink)
    } else if metadata.is_dir() {
        Ok(())
    } else {
        Err(PersonaError::NotRegular)
    }
}

#[cfg(unix)]
fn same_file(before: &Metadata, after: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    before.dev() == after.dev() && before.ino() == after.ino()
}

#[cfg(not(unix))]
fn same_file(_before: &Metadata, after: &Metadata) -> bool {
    after.is_file()
}

impl PersonaRoot {
    pub fn open(path: &Path) -> Result<Self, PersonaError> {
        check_dir(path)?;
        let canonical = fs::canonicalize(path).map_err(io_error)?;
        Ok(Self { canonical })
    }

    /// Canonical `path`, which must lie under the root and spell every
    /// component with its exact case: a case-insensitive volume (macOS) finds
    /// `Alpha.yaml` for `alpha.yaml` where Linux, the deployment, does not.
    fn resolve(&self, path: &Path) -> Result<PathBuf, PersonaError> {
        let resolved = fs::canonicalize(path).map_err(io_error)?;
        if !resolved.starts_with(&self.canonical) {
            Err(PersonaError::Escape)
        } else if resolved != path {
            Err(PersonaError::Missing)
        } else {
            Ok(resolved)
        }
    }

    fn read(&self, dir: Option<&str>, name: &str) -> Result<(String, Source), PersonaError> {
        let mut path = self.canonical.clone();
        if let Some(dir) = dir {
            path.push(dir);
            check_dir(&path)?;
        }
        path.push(name);
        let before = fs::symlink_metadata(&path).map_err(io_error)?;
        if before.file_type().is_symlink() {
            return Err(PersonaError::Symlink);
        }
        if !before.is_file() {
            return Err(PersonaError::NotRegular);
        }
        let resolved = self.resolve(&path)?;
        let (file, opened) = open_regular(&resolved, MAX_FILE_BYTES).map_err(refused)?;
        // Reject a file swapped between the checks above and the open.
        if !same_file(&before, &opened) {
            return Err(PersonaError::Escape);
        }
        let bytes = read_capped(file, MAX_FILE_BYTES).map_err(refused)?;
        let sha256 = digest(&SHA256, &bytes)
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let text = String::from_utf8(bytes).map_err(|_| PersonaError::InvalidUtf8)?;
        let source = Source {
            basename: name.to_owned(),
            sha256,
        };
        Ok((text, source))
    }

    pub fn load_catalog(&self) -> Result<Loaded<Catalog>, PersonaError> {
        let (text, source) = self.read(None, CATALOG_FILE)?;
        Ok(Loaded {
            value: parse_catalog(&text)?,
            source,
        })
    }

    /// Load `bundles/<id>.yaml`; the path is derived only from the validated ID.
    pub fn load_bundle(&self, id: &PersonaId) -> Result<Loaded<Bundle>, PersonaError> {
        let name = format!("{id}{EXTENSION}");
        let (text, source) = self.read(Some(BUNDLES_DIR), &name)?;
        Ok(Loaded {
            value: parse_bundle(&text, id)?,
            source,
        })
    }

    pub fn load_profile(&self, id: &ProfileId) -> Result<Loaded<Profile>, PersonaError> {
        if id.as_str() == EXAMPLE_PROFILE {
            return Err(PersonaError::Invalid(
                "the example profile is not selectable",
            ));
        }
        let name = format!("{id}{EXTENSION}");
        let (text, source) = self.read(Some(PROFILES_DIR), &name)?;
        Ok(Loaded {
            value: parse_profile(&text, id)?,
            source,
        })
    }

    /// Load every `profiles/<id>.yaml` except the tracked example. More than
    /// [`MAX_PROFILE_FILES`] candidates fails the whole load.
    pub fn load_profiles(&self) -> Result<ProfileSet, PersonaError> {
        let dir = self.canonical.join(PROFILES_DIR);
        match check_dir(&dir).and_then(|()| self.resolve(&dir).map(drop)) {
            Ok(()) => {}
            Err(PersonaError::Missing) => return Ok(ProfileSet::default()),
            Err(error) => return Err(error),
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(io_error)? {
            let name = entry.map_err(io_error)?.file_name();
            // Non-UTF-8 names cannot be derived from any ID, so they are never candidates.
            let Some(name) = name.to_str() else {
                continue;
            };
            if name
                .strip_suffix(EXTENSION)
                .is_none_or(|stem| stem == EXAMPLE_PROFILE)
            {
                continue;
            }
            if names.len() == MAX_PROFILE_FILES {
                return Err(PersonaError::TooManyProfiles);
            }
            names.push(name.to_owned());
        }
        names.sort();
        let mut set = ProfileSet::default();
        for name in names {
            let stem = &name[..name.len() - EXTENSION.len()];
            let loaded = ProfileId::parse(stem).and_then(|id| self.load_profile(&id));
            match loaded {
                Ok(profile) => {
                    set.readable.insert(profile.value.id.clone(), profile);
                }
                Err(error) => set.unreadable.push(ProfileIssue {
                    basename: name,
                    error,
                }),
            }
        }
        Ok(set)
    }
}
