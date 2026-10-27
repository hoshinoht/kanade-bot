//! Secret files: one line, at most 4 KiB, under fixed names in the secrets
//! directory (`KANADE_SECRETS_DIR`, `/run/secrets` in the image). Config only
//! names them; startup reads them and judges an absent one. Errors name the
//! file, never its contents.
//!
//! Admins may also save some secrets from the Setup page into a second,
//! bot-written directory (`KANADE_SAVED_SECRETS_DIR`, `/data/secrets` in the
//! image). It is read after the host directory, so a host file always wins.

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
/// The directory Setup saves secrets into; unset, nothing can be saved.
pub const SAVED_DIR_KEY: &str = "KANADE_SAVED_SECRETS_DIR";

/// The secrets an admin may save from Setup. The break-glass token, the edge
/// secret and the CA bundle stay host files: they guard sign-in and trust.
pub const SAVEABLE: [&str; 5] = [
    DISCORD_TOKEN,
    MODEL_API_KEY,
    DISCORD_CLIENT_SECRET,
    PUBLIC_DISCORD_CLIENT_SECRET,
    NEXON_API_KEY,
];

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
/// that cannot be checked (a directory without search permission) or a
/// symlink whose target is missing (it points outside the mounted directory)
/// refuses instead of silently turning its feature off.
pub fn present(path: &Path, label: &str) -> Result<bool, Error> {
    match std::fs::metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if std::fs::symlink_metadata(path).is_ok() {
                return Err(Error::Configuration(format!(
                    "{label} is a symlink to a missing file; use a regular file"
                )));
            }
            Ok(false)
        }
        Err(_) => Err(Error::Configuration(format!(
            "{label} could not be checked"
        ))),
    }
}

/// One fixed name in the secrets directory, and in the saved directory when
/// Setup may save it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretFile {
    path: PathBuf,
    saved: Option<PathBuf>,
    name: &'static str,
}

/// Where a secret was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Absent,
    /// A file in the host secrets directory (an admin cannot replace it).
    Host,
    /// Saved from Setup.
    Saved,
}

impl SecretFile {
    /// `name` in `KANADE_SECRETS_DIR` ([`DEFAULT_DIR`] when unset), then in
    /// `KANADE_SAVED_SECRETS_DIR` when it is set and `name` is [`SAVEABLE`].
    pub fn named(values: &BTreeMap<String, String>, name: &'static str) -> Self {
        let dir = values
            .get(DIR_KEY)
            .map(String::as_str)
            .filter(|dir| !dir.is_empty())
            .unwrap_or(DEFAULT_DIR);
        let mut file = Self::new(Path::new(dir), name);
        file.saved = values
            .get(SAVED_DIR_KEY)
            .map(String::as_str)
            .filter(|dir| !dir.is_empty() && SAVEABLE.contains(&name))
            .map(|dir| Path::new(dir).join(name));
        file
    }

    pub fn new(dir: &Path, name: &'static str) -> Self {
        Self {
            path: dir.join(name),
            saved: None,
            name,
        }
    }

    /// The host file's path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn label(&self) -> String {
        format!("secrets file `{}`", self.name)
    }

    /// Whether Setup may save this secret (a saved directory is configured).
    pub fn saveable(&self) -> bool {
        self.saved.is_some()
    }

    /// Where Setup saves it, when it may.
    #[cfg(any(test, feature = "test-support"))]
    pub fn saved_path(&self) -> Option<&Path> {
        self.saved.as_deref()
    }

    /// The host file when present, else the saved one when present.
    pub fn source(&self) -> Result<Source, Error> {
        if present(&self.path, &self.label())? {
            return Ok(Source::Host);
        }
        match &self.saved {
            Some(saved) if present(saved, &self.label())? => Ok(Source::Saved),
            _ => Ok(Source::Absent),
        }
    }

    /// The path a reader should open: the host file, else the saved one.
    pub fn located(&self) -> Result<Option<&Path>, Error> {
        Ok(match self.source()? {
            Source::Host => Some(&self.path),
            Source::Saved => self.saved.as_deref(),
            Source::Absent => None,
        })
    }

    /// Present; else refused, saying `why` the file is needed.
    pub fn read(&self, why: &str) -> Result<Redacted, Error> {
        self.read_optional()?.ok_or_else(|| self.missing(why))
    }

    /// `None` when absent; present but bad refuses.
    pub fn read_optional(&self) -> Result<Option<Redacted>, Error> {
        match self.located()? {
            Some(path) => read_secret(path, &self.label()).map(|secret| Some(Redacted(secret))),
            None => Ok(None),
        }
    }

    pub fn present(&self) -> Result<bool, Error> {
        Ok(self.source()? != Source::Absent)
    }

    pub fn missing(&self, why: &str) -> Error {
        let dir = self.path.parent().unwrap_or(Path::new(""));
        let saved = if self.saved.is_some() {
            " (or save it in Setup)"
        } else {
            ""
        };
        Error::Configuration(format!(
            "{} is missing from {}{saved}: {why}",
            self.label(),
            dir.display()
        ))
    }

    /// Saves `value` into the saved directory (temp file, then rename; 0600).
    /// Refused when this secret cannot be saved, a host file exists (it wins,
    /// so a saved copy would never be read), or `value` is not one line.
    pub fn save(&self, value: &str) -> Result<(), SaveError> {
        let target = self.saved.as_deref().ok_or(SaveError::NotSaveable)?;
        if present(&self.path, &self.label()).map_err(|_| SaveError::Unavailable)? {
            return Err(SaveError::OnHost);
        }
        // As a check sees it: surrounding whitespace is a paste artefact.
        let value = value.trim();
        if value.is_empty()
            || value.len() as u64 > MAX_SECRET_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(SaveError::Invalid);
        }
        let dir = target.parent().ok_or(SaveError::Unavailable)?;
        prepare_saved_dir(dir).map_err(|_| SaveError::Unavailable)?;
        write_atomically(dir, self.name, value).map_err(|_| SaveError::Unavailable)
    }
}

/// Why a secret could not be saved. Never carries the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SaveError {
    #[error("this secret can only be a file in the secrets directory")]
    NotSaveable,
    #[error("a file in the secrets directory already sets this secret")]
    OnHost,
    #[error("a secret is one non-empty line of at most 4 KiB")]
    Invalid,
    #[error("the saved secrets directory is not writable")]
    Unavailable,
}

/// Creates the saved directory 0700, or checks an existing one is a private
/// directory (not a symlink, no group or other access).
pub fn prepare_saved_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() && meta.permissions().mode() & 0o077 == 0 => Ok(()),
        Ok(_) => Err(std::io::Error::other(
            "the saved secrets directory must be a private directory",
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            std::fs::DirBuilder::new().mode(0o700).create(dir)
        }
        Err(error) => Err(error),
    }
}

fn write_atomically(dir: &Path, name: &str, value: &str) -> std::io::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let temp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(value.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temp, dir.join(name))?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
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
    fn a_dangling_symlink_refuses_instead_of_reading_as_absent() {
        let dir = Dir::new();
        let file = dir.file();
        std::os::unix::fs::symlink(dir.0.join("elsewhere-on-the-host"), file.path()).unwrap();
        assert_eq!(
            file.read_optional().unwrap_err().to_string(),
            "secrets file `admin_token` is a symlink to a missing file; use a regular file"
        );
    }

    fn host_and_saved(host: &Dir, saved: &Dir, name: &'static str) -> SecretFile {
        let values = BTreeMap::from([
            (DIR_KEY.to_owned(), host.0.display().to_string()),
            (
                SAVED_DIR_KEY.to_owned(),
                saved.0.join("secrets").display().to_string(),
            ),
        ]);
        SecretFile::named(&values, name)
    }

    #[test]
    fn a_saved_secret_is_read_after_the_host_file_which_always_wins() {
        use std::os::unix::fs::PermissionsExt;
        let (host, saved) = (Dir::new(), Dir::new());
        let file = host_and_saved(&host, &saved, MODEL_API_KEY);
        assert!(file.saveable());
        assert_eq!(file.source().unwrap(), Source::Absent);
        file.save("saved-key\n").unwrap();
        assert_eq!(file.source().unwrap(), Source::Saved);
        assert_eq!(file.read("needed").unwrap().expose(), "saved-key");
        let dir = saved.0.join("secrets");
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!((mode(&dir), mode(&dir.join(MODEL_API_KEY))), (0o700, 0o600));
        // Saving again replaces the value; no temp file is left behind.
        // A pasted value's surrounding whitespace is dropped, as a check drops it.
        file.save("  second-key \r\n").unwrap();
        assert_eq!(file.read("needed").unwrap().expose(), "second-key");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        // A host file wins, and Setup cannot save over it.
        std::fs::write(file.path(), "host-key\n").unwrap();
        assert_eq!(file.source().unwrap(), Source::Host);
        assert_eq!(file.read("needed").unwrap().expose(), "host-key");
        assert_eq!(file.save("third-key"), Err(SaveError::OnHost));
    }

    #[test]
    fn only_saveable_secrets_with_a_saved_directory_can_be_saved() {
        let (host, saved) = (Dir::new(), Dir::new());
        let token = host_and_saved(&host, &saved, ADMIN_TOKEN);
        assert!(!token.saveable());
        assert_eq!(
            token.save("x".repeat(40).as_str()),
            Err(SaveError::NotSaveable)
        );
        let bare = SecretFile::named(&BTreeMap::new(), DISCORD_TOKEN);
        assert_eq!(bare.save("value"), Err(SaveError::NotSaveable));
        let key = host_and_saved(&host, &saved, NEXON_API_KEY);
        for bad in ["", "\n", "a\nb", "tab\there", &"x".repeat(4097)] {
            assert_eq!(key.save(bad), Err(SaveError::Invalid), "{bad:?}");
        }
        assert_eq!(key.source().unwrap(), Source::Absent);
    }

    #[test]
    fn a_shared_saved_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (host, saved) = (Dir::new(), Dir::new());
        let file = host_and_saved(&host, &saved, MODEL_API_KEY);
        let dir = saved.0.join("secrets");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(file.save("key"), Err(SaveError::Unavailable));
        assert_eq!(file.source().unwrap(), Source::Absent);
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
