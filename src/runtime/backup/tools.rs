//! `kanade backup keygen|encrypt|decrypt`: the age key and file tools the
//! runbook needs, since the runtime image has no shell or `age` binary.
//! None of them touches the store.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use age::secrecy::ExposeSecret;

use super::crypt::{self, Identities, Recipients};
use crate::runtime::error::Error;

/// A new 0600 file at `path` (`what` names it in errors); never overwrites.
fn create_private(path: &Path, what: &str) -> Result<File, Error> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => {
                Error::Configuration(format!("{what} already exists; it is never overwritten"))
            }
            _ => Error::Configuration(format!("{what} could not be created ({error})")),
        })
}

/// Write a new identity to `out` (0600) and return its public key.
///
/// # Errors
/// `out` exists or cannot be written; a partial file is removed.
pub fn keygen(out: &Path) -> Result<String, Error> {
    let (secret, public) = crypt::generate();
    let mut file = create_private(out, "backup keygen: --out")?;
    file.write_all(secret.expose_secret().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            let _ = std::fs::remove_file(out);
            Error::Unavailable(format!(
                "backup keygen: --out could not be written ({error})"
            ))
        })?;
    Ok(public)
}

/// Encrypt `input` to the recipients in `recipients_file` onto `output`.
///
/// # Errors
/// A bad recipients file (before any output), or an I/O failure.
pub fn encrypt(
    recipients_file: &Path,
    label: &str,
    input: impl Read,
    output: impl Write,
) -> Result<(), Error> {
    let recipients = Recipients::load(recipients_file, label)?;
    recipients
        .encrypt(input, output)
        .map_err(|error| Error::Unavailable(format!("backup encrypt: {error}")))
}

/// Decrypt the age file `input` with `identity` into a new 0600 `output`.
/// A wrong identity or a foreign file fails before `output` is created; a
/// damaged or truncated file removes the partial `output`.
///
/// # Errors
/// As above, or `output` exists (never overwritten).
pub fn decrypt(identity: &Path, input: &Path, output: &Path) -> Result<(), Error> {
    let identities = Identities::load(identity, "backup decrypt: --identity")?;
    let sealed = File::open(input).map_err(|error| {
        Error::Configuration(format!("backup decrypt: --in could not be read ({error})"))
    })?;
    let mut plain = identities
        .decrypt(sealed)
        .map_err(|error| Error::Configuration(format!("backup decrypt: {error}")))?;
    let mut file = create_private(output, "backup decrypt: --out")?;
    io::copy(&mut plain, &mut file)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            let _ = std::fs::remove_file(output);
            let what = match error.kind() {
                io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof => {
                    "--in is damaged or truncated"
                }
                _ => "--out could not be written",
            };
            Error::Configuration(format!("backup decrypt: {what}; nothing kept ({error})"))
        })
}
