use std::path::Path;

/// A refused file: its path and the problem, never its content.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {problem}")]
pub struct LoadError {
    pub file: String,
    pub problem: String,
}

impl LoadError {
    pub fn new(file: &Path, problem: impl Into<String>) -> Self {
        Self {
            file: file.display().to_string(),
            problem: problem.into(),
        }
    }
}
