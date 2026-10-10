//! age (X25519) encryption for backups. A recipients file holds only public
//! keys (`age1…`), the one piece of key material the host and containers
//! need; an identity file (`AGE-SECRET-KEY-1…`) is read only by
//! `kanade backup decrypt` when restoring. Errors name a line by number,
//! never its content.

use std::{
    fs::File,
    io::{self, BufReader, Read, Write},
    path::Path,
    str::FromStr,
};

use age::{
    secrecy::{ExposeSecret, SecretString},
    stream::StreamReader,
    x25519,
};

use crate::runtime::error::Error;

/// Far above any real key list; refuses a setting aimed at a large file.
const MAX_KEY_FILE_BYTES: u64 = 64 * 1024;
const SECRET_PREFIX: &str = "AGE-SECRET-KEY-";

/// The X25519 public keys backups are encrypted to.
#[derive(Clone)]
pub struct Recipients(Vec<x25519::Recipient>);

impl Recipients {
    /// One `age1…` key per line; blank lines and `#` comments are skipped.
    ///
    /// # Errors
    /// Unreadable or oversized, a line that is not a recipient (a private
    /// identity is called out), or no keys at all.
    pub fn load(path: &Path, label: &str) -> Result<Self, Error> {
        let text = read_key_file(path, label, "recipients")?;
        let mut keys = Vec::new();
        for (number, line) in key_lines(text.expose_secret()) {
            if line
                .get(..SECRET_PREFIX.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(SECRET_PREFIX))
            {
                return Err(Error::Configuration(format!(
                    "{label}: line {number} is a private age identity; a recipients file holds only public age1… keys"
                )));
            }
            keys.push(x25519::Recipient::from_str(line).map_err(|_| {
                Error::Configuration(format!(
                    "{label}: line {number} is not an age X25519 recipient (age1…)"
                ))
            })?);
        }
        if keys.is_empty() {
            return Err(Error::Configuration(format!(
                "{label} holds no age recipients"
            )));
        }
        Ok(Self(keys))
    }

    /// Encrypt all of `input` to every recipient as an age file on `output`.
    ///
    /// # Errors
    /// Reading `input` or writing `output` failed; `output` is then partial.
    pub fn encrypt(&self, mut input: impl Read, output: impl Write) -> io::Result<()> {
        let encryptor =
            age::Encryptor::with_recipients(self.0.iter().map(|key| key as &dyn age::Recipient))
                .map_err(io::Error::other)?;
        let mut writer = encryptor.wrap_output(output)?;
        io::copy(&mut input, &mut writer)?;
        writer.finish()?.flush()
    }
}

/// X25519 private keys, for decrypting a backup.
pub struct Identities(Vec<x25519::Identity>);

impl Identities {
    /// One `AGE-SECRET-KEY-1…` per line (an `age-keygen` or
    /// `kanade backup keygen` file); blank lines and `#` comments are skipped.
    ///
    /// # Errors
    /// Unreadable or oversized, a line that is not an identity, or none.
    pub fn load(path: &Path, label: &str) -> Result<Self, Error> {
        let text = read_key_file(path, label, "identity")?;
        let keys = key_lines(text.expose_secret())
            .map(|(number, line)| {
                x25519::Identity::from_str(line).map_err(|_| {
                    Error::Configuration(format!(
                        "{label}: line {number} is not an age X25519 identity (AGE-SECRET-KEY-1…)"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if keys.is_empty() {
            return Err(Error::Configuration(format!(
                "{label} holds no age identities"
            )));
        }
        Ok(Self(keys))
    }

    /// A plaintext reader over the age file `input`. The header is checked
    /// here, so a foreign file or a non-matching identity fails before any
    /// plaintext is produced; the reader itself fails on damage or truncation.
    ///
    /// # Errors
    /// Not an age file, or no identity matches its recipients.
    pub fn decrypt<R: Read>(&self, input: R) -> Result<StreamReader<BufReader<R>>, Error> {
        let decryptor = age::Decryptor::new_buffered(BufReader::new(input))
            .map_err(|error| Error::Configuration(format!("not a readable age file ({error})")))?;
        decryptor
            .decrypt(self.0.iter().map(|key| key as &dyn age::Identity))
            .map_err(|error| match error {
                age::DecryptError::NoMatchingKeys => Error::Configuration(
                    "the identity matches none of this file's recipients".into(),
                ),
                error => Error::Configuration(format!("cannot decrypt ({error})")),
            })
    }
}

/// A new identity file's text (secret) and its public key (`age1…`).
pub fn generate() -> (SecretString, String) {
    let identity = x25519::Identity::generate();
    let public = identity.to_public().to_string();
    let text = format!(
        "# kanade backup identity: keep it off the backup host.\n# public key: {public}\n{}\n",
        identity.to_string().expose_secret()
    );
    (SecretString::from(text), public)
}

fn read_key_file(path: &Path, label: &str, kind: &str) -> Result<SecretString, Error> {
    let refused = || Error::Configuration(format!("{label} must name a readable {kind} file"));
    let file = File::open(path).map_err(|_| refused())?;
    let metadata = file.metadata().map_err(|_| refused())?;
    if !metadata.is_file() || metadata.len() > MAX_KEY_FILE_BYTES {
        return Err(refused());
    }
    let mut text = String::new();
    file.take(MAX_KEY_FILE_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| refused())?;
    Ok(SecretString::from(text))
}

/// Non-blank, non-comment lines, trimmed, with 1-based line numbers.
fn key_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn with(text: &str) -> Self {
            let path = std::env::temp_dir().join(format!("kanade-age-{}", uuid::Uuid::new_v4()));
            std::fs::write(&path, text).unwrap();
            Self(path)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn message(result: Result<Recipients, Error>) -> String {
        result.err().expect("refused").to_string()
    }

    #[test]
    fn recipients_files_take_comments_and_refuse_anything_else() {
        let (secret, public) = generate();
        let (_, other) = generate();
        let good = Temp::with(&format!("# ops laptop\n\n  {public}  \n{other}\n"));
        assert_eq!(Recipients::load(&good.0, "R").unwrap().0.len(), 2);

        let empty = Temp::with("# nothing yet\n\n");
        assert_eq!(
            message(Recipients::load(&empty.0, "R")),
            "R holds no age recipients"
        );
        let garbage = Temp::with(&format!("{public}\nnot-a-key\n"));
        assert_eq!(
            message(Recipients::load(&garbage.0, "R")),
            "R: line 2 is not an age X25519 recipient (age1…)"
        );
        // A pasted private key is refused without echoing it.
        let pasted = Temp::with(secret.expose_secret());
        let refused = message(Recipients::load(&pasted.0, "R"));
        assert_eq!(
            refused,
            "R: line 3 is a private age identity; a recipients file holds only public age1… keys"
        );
        let missing = good.0.with_extension("missing");
        assert_eq!(
            message(Recipients::load(&missing, "R")),
            "R must name a readable recipients file"
        );
        let dir = std::env::temp_dir();
        assert_eq!(
            message(Recipients::load(&dir, "R")),
            "R must name a readable recipients file"
        );
    }

    #[test]
    fn a_generated_identity_decrypts_what_its_public_key_encrypts() {
        let (secret, public) = generate();
        let recipients = Temp::with(&public);
        let identity = Temp::with(secret.expose_secret());
        let plain: Vec<u8> = (0..=255u8).cycle().take(200_000).collect();
        let mut sealed = Vec::new();
        Recipients::load(&recipients.0, "R")
            .unwrap()
            .encrypt(plain.as_slice(), &mut sealed)
            .unwrap();
        assert!(sealed.starts_with(b"age-encryption.org/v1\n"));
        let mut opened = Vec::new();
        Identities::load(&identity.0, "I")
            .unwrap()
            .decrypt(sealed.as_slice())
            .unwrap()
            .read_to_end(&mut opened)
            .unwrap();
        assert_eq!(opened, plain);

        let (stranger, _) = generate();
        let stranger = Temp::with(stranger.expose_secret());
        let refused = Identities::load(&stranger.0, "I")
            .unwrap()
            .decrypt(sealed.as_slice())
            .err()
            .expect("wrong identity");
        assert_eq!(
            refused.to_string(),
            "the identity matches none of this file's recipients"
        );
        let public_only = Temp::with(&public);
        assert_eq!(
            Identities::load(&public_only.0, "I")
                .err()
                .expect("refused")
                .to_string(),
            "I: line 1 is not an age X25519 identity (AGE-SECRET-KEY-1…)"
        );
    }
}
