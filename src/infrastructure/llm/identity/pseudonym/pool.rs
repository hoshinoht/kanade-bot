use std::{fmt, sync::Arc};

const CURATED: &str = include_str!("names.txt");

/// Candidate pseudonyms. Only names of at least 3 ASCII letters that do not end
/// in `s` are kept (decoding strips one trailing `s` for plurals and
/// possessives); duplicates are dropped case-insensitively.
#[derive(Clone)]
pub struct NamePool {
    names: Arc<[String]>,
}

impl NamePool {
    /// The tracked list (`pseudonym/names.txt`).
    pub fn curated() -> Self {
        Self::from_names(
            CURATED
                .lines()
                .map(str::trim)
                .filter(|line| !line.starts_with('#')),
        )
    }

    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut kept: Vec<String> = Vec::new();
        for name in names {
            let name = name.as_ref().trim();
            let usable = name.len() >= 3
                && name.bytes().all(|b| b.is_ascii_alphabetic())
                && !name.to_ascii_lowercase().ends_with('s');
            if usable && !kept.iter().any(|known| known.eq_ignore_ascii_case(name)) {
                kept.push(name.to_owned());
            }
        }
        Self { names: kept.into() }
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl fmt::Debug for NamePool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NamePool")
            .field("names", &self.names.len())
            .finish()
    }
}
