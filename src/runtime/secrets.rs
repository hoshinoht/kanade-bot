//! Secret files: one line, at most 4 KiB. Errors name the variable, never the
//! path contents.

use std::{fmt, path::Path};

use super::error::Error;

const MAX_SECRET_BYTES: u64 = 4096;

pub fn read_secret(path: &Path, variable: &str) -> Result<String, Error> {
    let refused = || Error::Configuration(format!("{variable} must name a readable secret file"));
    let metadata = std::fs::metadata(path).map_err(|_| refused())?;
    if !metadata.is_file() || metadata.len() > MAX_SECRET_BYTES {
        return Err(refused());
    }
    let text = std::fs::read_to_string(path).map_err(|_| refused())?;
    let secret = text.trim_end_matches(['\r', '\n']).to_owned();
    if secret.is_empty() || secret.chars().any(char::is_control) {
        return Err(Error::Configuration(format!(
            "{variable} must contain one non-empty line"
        )));
    }
    Ok(secret)
}

/// A loaded secret whose `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn read(path: &Path, variable: &str) -> Result<Self, Error> {
        read_secret(path, variable).map(Self)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Redacted(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_one_line_and_redacted() {
        let path = std::env::temp_dir().join(format!("kanade-secret-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "hunter2-token\r\n").unwrap();
        let secret = Redacted::read(&path, "KANADE_X_FILE").unwrap();
        assert_eq!(secret.expose(), "hunter2-token");
        assert!(!format!("{secret:?}").contains("hunter2"));
        std::fs::write(&path, "a\nb").unwrap();
        assert_eq!(
            read_secret(&path, "KANADE_X_FILE").unwrap_err().to_string(),
            "KANADE_X_FILE must contain one non-empty line"
        );
        std::fs::write(&path, "x".repeat(4097)).unwrap();
        assert_eq!(
            read_secret(&path, "KANADE_X_FILE").unwrap_err().to_string(),
            "KANADE_X_FILE must name a readable secret file"
        );
        std::fs::remove_file(path).unwrap();
    }
}
