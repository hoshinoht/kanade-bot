//! Contained reads of the v5 layout only: `catalog.yaml`, `bundles/`, `profiles/`.

use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
};

use ring::digest::{SHA256, digest};

use super::{
    PersonaError,
    id::{EXAMPLE_PROFILE, PersonaId, ProfileId},
    schema::{Bundle, Catalog, Profile, parse_bundle, parse_catalog, parse_profile},
};

const CATALOG_FILE: &str = "catalog.yaml";
const BUNDLES_DIR: &str = "bundles";
const PROFILES_DIR: &str = "profiles";
const EXTENSION: &str = ".yaml";
const MAX_FILE_BYTES: u64 = 256 * 1024;

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
        let resolved = fs::canonicalize(&path).map_err(io_error)?;
        if !resolved.starts_with(&self.canonical) {
            return Err(PersonaError::Escape);
        }
        if before.len() > MAX_FILE_BYTES {
            return Err(PersonaError::TooLarge);
        }
        let file = File::open(&resolved).map_err(io_error)?;
        // Reject a file swapped between the checks above and the open.
        if !same_file(&before, &file.metadata().map_err(io_error)?) {
            return Err(PersonaError::Escape);
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(PersonaError::TooLarge);
        }
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

    /// Load every `profiles/<id>.yaml` except the tracked example.
    pub fn load_profiles(&self) -> Result<ProfileSet, PersonaError> {
        let dir = self.canonical.join(PROFILES_DIR);
        match check_dir(&dir) {
            Ok(()) => {}
            Err(PersonaError::Missing) => return Ok(ProfileSet::default()),
            Err(error) => return Err(error),
        }
        let mut names = Vec::new();
        for entry in fs::read_dir(&dir).map_err(io_error)? {
            let name = entry.map_err(io_error)?.file_name();
            // Non-UTF-8 names cannot be derived from any ID, so they are never candidates.
            if let Some(name) = name.to_str() {
                names.push(name.to_owned());
            }
        }
        names.sort();
        let mut set = ProfileSet::default();
        for name in names {
            let Some(stem) = name.strip_suffix(EXTENSION) else {
                continue;
            };
            if stem == EXAMPLE_PROFILE {
                continue;
            }
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
