//! Secret files: one line, at most 4 KiB, under fixed names in the secrets
//! directory (`KANADE_SECRETS_DIR`, `/run/secrets` in the image). Config only
//! names them; startup reads them and judges an absent one. Errors name the
//! file, never its contents.

use std::{
    collections::BTreeMap,
    fmt,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use super::error::Error;

const MAX_SECRET_BYTES: u64 = 4096;

/// The secrets directory; the image sets it to [`DEFAULT_DIR`].
pub const DIR_KEY: &str = "KANADE_SECRETS_DIR";
pub const DEFAULT_DIR: &str = "/run/secrets";

/// Always (serve and `kanade ctl emojis`).
pub const DISCORD_TOKEN: &str = "discord_token";
/// Break-glass sign-in when present; required by `kanade portal`/`sessions`.
pub const ADMIN_TOKEN: &str = "admin_token";
/// The model gateway's bearer key when present (`models.base_url` set).
pub const MODEL_API_KEY: &str = "model_api_key";
/// A PEM bundle (or one DER certificate) replacing the compiled roots for
/// the model gateway when present.
pub const MODEL_CA: &str = "model_ca.pem";
/// Required by `admin.discord_client_id`.
pub const DISCORD_CLIENT_SECRET: &str = "discord_client_secret";
/// Required by `public.discord_client_id`.
pub const PUBLIC_DISCORD_CLIENT_SECRET: &str = "public_discord_client_secret";
/// The edge's proof (`X-Kanade-Edge-Auth`) when present with
/// `KANADE_TRUSTED_PROXY`; required by `admin.tailscale_logins`.
pub const EDGE_SECRET: &str = "edge_secret";
/// The NEXON Open API key when present (module `character_data`).
pub const NEXON_API_KEY: &str = "nexon_api_key";

pub fn read_secret(path: &Path, label: &str) -> Result<String, Error> {
    let refused =
        || Error::Configuration(format!("{label} must be a readable file of at most 4 KiB"));
    let metadata = std::fs::metadata(path).map_err(|_| refused())?;
    if !metadata.is_file() || metadata.len() > MAX_SECRET_BYTES {
        return Err(refused());
    }
    let text = std::fs::read_to_string(path).map_err(|_| refused())?;
    let secret = text.trim_end_matches(['\r', '\n']).to_owned();
    if secret.is_empty() || secret.chars().any(char::is_control) {
        return Err(Error::Configuration(format!(
            "{label} must contain one non-empty line"
        )));
    }
    Ok(secret)
}

/// Whether anything is at `path`. Only "not found" counts as absent: a file
/// that cannot be checked (a directory without search permission) refuses
/// instead of silently turning its feature off.
pub fn present(path: &Path, label: &str) -> Result<bool, Error> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::Configuration(format!(
            "{label} could not be checked"
        ))),
    }
}

/// One fixed name in the secrets directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretFile {
    path: PathBuf,
    name: &'static str,
}

impl SecretFile {
    /// `name` in `KANADE_SECRETS_DIR` ([`DEFAULT_DIR`] when unset).
    pub fn named(values: &BTreeMap<String, String>, name: &'static str) -> Self {
        let dir = values
            .get(DIR_KEY)
            .map(String::as_str)
            .filter(|dir| !dir.is_empty())
            .unwrap_or(DEFAULT_DIR);
        Self::new(Path::new(dir), name)
    }

    pub fn new(dir: &Path, name: &'static str) -> Self {
        Self {
            path: dir.join(name),
            name,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn label(&self) -> String {
        format!("secrets file `{}`", self.name)
    }

    /// Present; else refused, saying `why` the file is needed.
    pub fn read(&self, why: &str) -> Result<Redacted, Error> {
        self.read_optional()?.ok_or_else(|| self.missing(why))
    }

    /// `None` when absent; present but bad refuses.
    pub fn read_optional(&self) -> Result<Option<Redacted>, Error> {
        if !self.present()? {
            return Ok(None);
        }
        read_secret(&self.path, &self.label()).map(|secret| Some(Redacted(secret)))
    }

    pub fn present(&self) -> Result<bool, Error> {
        present(&self.path, &self.label())
    }

    pub fn missing(&self, why: &str) -> Error {
        let dir = self.path.parent().unwrap_or(Path::new(""));
        Error::Configuration(format!(
            "{} is missing from {}: {why}",
            self.label(),
            dir.display()
        ))
    }
}

/// A loaded secret whose `Debug` output is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn read(path: &Path, label: &str) -> Result<Self, Error> {
        read_secret(path, label).map(Self)
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
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("kanade-secrets-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn file(&self) -> SecretFile {
            let values = BTreeMap::from([(DIR_KEY.to_owned(), self.0.display().to_string())]);
            SecretFile::named(&values, ADMIN_TOKEN)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn secrets_are_one_line_and_redacted() {
        let dir = Dir::new();
        let file = dir.file();
        std::fs::write(file.path(), "hunter2-token\r\n").unwrap();
        let secret = file.read("needed").unwrap();
        assert_eq!(secret.expose(), "hunter2-token");
        assert!(!format!("{secret:?}").contains("hunter2"));
        std::fs::write(file.path(), "a\nb").unwrap();
        assert_eq!(
            file.read_optional().unwrap_err().to_string(),
            "secrets file `admin_token` must contain one non-empty line"
        );
        std::fs::write(file.path(), "").unwrap();
        assert_eq!(
            file.read_optional().unwrap_err().to_string(),
            "secrets file `admin_token` must contain one non-empty line"
        );
        std::fs::write(file.path(), "x".repeat(4097)).unwrap();
        let error = file.read_optional().unwrap_err().to_string();
        assert_eq!(
            error,
            "secrets file `admin_token` must be a readable file of at most 4 KiB"
        );
        assert!(!error.contains("xxx"));
    }

    #[test]
    fn absent_files_are_none_or_refused_by_the_feature_that_needs_them() {
        let dir = Dir::new();
        let file = dir.file();
        assert_eq!(file.read_optional().unwrap(), None);
        assert_eq!(
            file.read("this command signs in with it")
                .unwrap_err()
                .to_string(),
            format!(
                "secrets file `admin_token` is missing from {}: this command signs in with it",
                dir.0.display()
            )
        );
        // A directory where the file should be is present but unreadable.
        std::fs::create_dir(file.path()).unwrap();
        assert_eq!(
            file.read_optional().unwrap_err().to_string(),
            "secrets file `admin_token` must be a readable file of at most 4 KiB"
        );
    }

    #[test]
    fn an_unsearchable_directory_refuses_instead_of_reading_as_absent() {
        let dir = Dir::new();
        let file = dir.file();
        std::fs::write(file.path(), "secret-value-sentinel\n").unwrap();
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores permission bits, so the check cannot be shown there.
        if std::fs::metadata(file.path()).is_ok() {
            return;
        }
        let error = file.read_optional().unwrap_err().to_string();
        assert_eq!(error, "secrets file `admin_token` could not be checked");
        assert!(!error.contains("sentinel"));
    }

    #[test]
    fn the_directory_defaults_to_the_image_path() {
        let file = SecretFile::named(&BTreeMap::new(), DISCORD_TOKEN);
        assert_eq!(file.path(), Path::new("/run/secrets/discord_token"));
        let empty = BTreeMap::from([(DIR_KEY.to_owned(), String::new())]);
        assert_eq!(
            SecretFile::named(&empty, DISCORD_TOKEN).path(),
            Path::new("/run/secrets/discord_token")
        );
    }
}
