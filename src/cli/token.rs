//! `kanade token rotate --out FILE [--force]`: a new break-glass admin
//! token (32 random bytes, base64url, one line) written 0600. Never follows
//! a symbolic link, never overwrites without `--force` (then writes a
//! temporary file beside it and renames it over). Prints only the path; the
//! bot reads the file at startup, so a restart applies it and ends the
//! sessions signed in with the old token. No store, no API.

use std::{
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use crate::{app::crypto::random_token, runtime::error::Error};

pub const USAGE: &str = "token rotate --out FILE [--force]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub out: PathBuf,
    pub force: bool,
}

fn usage() -> Error {
    Error::Usage(format!("usage: kanade {USAGE}"))
}

pub fn parse(arguments: &[String]) -> Result<Args, Error> {
    let (verb, rest) = arguments.split_first().ok_or_else(usage)?;
    if verb != "rotate" {
        return Err(usage());
    }
    let ([force], [out]) = super::options(rest, ["--force"], ["--out"], usage)?;
    Ok(Args {
        out: out.map(PathBuf::from).ok_or_else(usage)?,
        force,
    })
}

/// Write a new token to `args.out` and return the line to print.
///
/// # Errors
/// `out` is a symbolic link or not a regular file, exists without `--force`,
/// or cannot be written (a partial file is removed).
pub fn rotate(args: &Args) -> Result<String, Error> {
    let out = &args.out;
    let exists = match fs::symlink_metadata(out) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(Error::Configuration(
                "token rotate: --out is a symbolic link; it is never followed or replaced".into(),
            ));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(Error::Configuration(
                "token rotate: --out exists and is not a regular file".into(),
            ));
        }
        Ok(_) if !args.force => {
            return Err(Error::Configuration(
                "token rotate: --out already exists; pass --force to replace it".into(),
            ));
        }
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => {
            return Err(Error::Configuration(format!(
                "token rotate: --out cannot be checked ({})",
                error.kind()
            )));
        }
    };
    let token = random_token()
        .ok_or_else(|| Error::Unavailable("token rotate: no system randomness".into()))?;
    let line = format!("{token}\n");
    if exists {
        let name = out
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let temp = out.with_file_name(format!(".{name}.tmp-{}", uuid::Uuid::new_v4()));
        write_new(&temp, &line)?;
        fs::rename(&temp, out).map_err(|error| {
            let _ = fs::remove_file(&temp);
            Error::Unavailable(format!(
                "token rotate: --out could not be replaced ({})",
                error.kind()
            ))
        })?;
    } else {
        write_new(out, &line)?;
    }
    if let Some(dir) = out.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        let _ = File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(format!(
        "token rotate: wrote {} (0600); restart the bot to apply it (sessions signed in with the old token end)",
        out.display()
    ))
}

/// A new 0600 file holding `line`, never through a symbolic link; removed
/// again if writing fails.
fn write_new(path: &Path, line: &str) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| match error.kind() {
            ErrorKind::AlreadyExists => Error::Configuration(
                "token rotate: --out appeared meanwhile; it is never overwritten without --force"
                    .into(),
            ),
            kind => {
                Error::Configuration(format!("token rotate: --out could not be created ({kind})"))
            }
        })?;
    file.write_all(line.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            let _ = fs::remove_file(path);
            Error::Unavailable(format!(
                "token rotate: --out could not be written ({})",
                error.kind()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Result<Args, Error> {
        parse(&text.split(' ').map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn rotate_takes_out_and_an_optional_force() {
        assert_eq!(
            args("rotate --out /s/admin_token").unwrap(),
            Args {
                out: "/s/admin_token".into(),
                force: false
            }
        );
        assert_eq!(
            args("rotate --force --out t").unwrap(),
            Args {
                out: "t".into(),
                force: true
            }
        );
        for bad in [
            "rotate",
            "rotate --out",
            "rotate --force",
            "rotate --out a --out b",
            "rotate --out a --force --force",
            "show --out a",
            "rotate --out a extra",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }
}
