use std::{fmt, path::Path};

/// A refused file: its path and the problem, never its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadError {
    pub file: String,
    pub problem: String,
}

impl LoadError {
    pub(crate) fn new(file: &Path, problem: impl Into<String>) -> Self {
        Self {
            file: file.display().to_string(),
            problem: problem.into(),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.file, self.problem)
    }
}

impl std::error::Error for LoadError {}
