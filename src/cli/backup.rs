//! `kanade backup …`: a store snapshot (default), or the age tools that
//! generate a key pair, encrypt a stream and decrypt a backup.

use std::path::PathBuf;

use crate::runtime::error::Error;

/// The longest accepted `--name`, leaving room for `.age.manifest.json`.
const MAX_NAME: usize = 200;

#[derive(Debug, PartialEq, Eq)]
pub enum Args {
    /// `backup [--name FILE]`: a snapshot named `FILE` (a plain file name) in
    /// `KANADE_BACKUP_DIR`, or a timestamped default name.
    Snapshot { name: Option<String> },
    /// `backup keygen --out FILE`: a new identity (0600), public key on stdout.
    Keygen { out: PathBuf },
    /// `backup encrypt [--recipients FILE]`: stdin to stdout; the default is
    /// `KANADE_BACKUP_RECIPIENTS_FILE`.
    Encrypt { recipients: Option<PathBuf> },
    /// `backup decrypt --identity FILE --in FILE --out FILE`.
    Decrypt {
        identity: PathBuf,
        input: PathBuf,
        output: PathBuf,
    },
}

pub const USAGE: &str = "backup [--name FILE]|backup keygen --out FILE|backup encrypt [--recipients FILE]|backup decrypt --identity FILE --in FILE --out FILE";

fn usage() -> Error {
    Error::Usage(format!("usage: kanade {USAGE}"))
}

pub fn parse(arguments: &[String]) -> Result<Args, Error> {
    match arguments.split_first() {
        Some((tool, rest)) if tool == "keygen" => {
            let [out] = flags(rest, ["--out"])?;
            Ok(Args::Keygen {
                out: required(out)?,
            })
        }
        Some((tool, rest)) if tool == "encrypt" => {
            let [recipients] = flags(rest, ["--recipients"])?;
            Ok(Args::Encrypt {
                recipients: recipients.map(PathBuf::from),
            })
        }
        Some((tool, rest)) if tool == "decrypt" => {
            let [identity, input, output] = flags(rest, ["--identity", "--in", "--out"])?;
            Ok(Args::Decrypt {
                identity: required(identity)?,
                input: required(input)?,
                output: required(output)?,
            })
        }
        _ => snapshot(arguments),
    }
}

/// `--flag value` pairs in any order, each at most once and non-empty.
fn flags<'a, const N: usize>(
    arguments: &'a [String],
    names: [&str; N],
) -> Result<[Option<&'a str>; N], Error> {
    let mut values = [None; N];
    let mut rest = arguments;
    while let [flag, value, tail @ ..] = rest {
        let slot = names
            .iter()
            .position(|name| name == flag)
            .ok_or_else(usage)?;
        if values[slot].is_some() || value.is_empty() {
            return Err(usage());
        }
        values[slot] = Some(value.as_str());
        rest = tail;
    }
    if rest.is_empty() {
        Ok(values)
    } else {
        Err(usage())
    }
}

fn required(value: Option<&str>) -> Result<PathBuf, Error> {
    value.map(PathBuf::from).ok_or_else(usage)
}

fn snapshot(arguments: &[String]) -> Result<Args, Error> {
    match arguments {
        [] => Ok(Args::Snapshot { name: None }),
        [flag, name] if flag == "--name" => {
            let plain = !name.is_empty()
                && name.len() <= MAX_NAME
                && !name.starts_with('.')
                && !name.ends_with(".manifest.json")
                && !name.ends_with(".age")
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
            if !plain {
                return Err(Error::Usage(
                    "--name must be a plain file name ([A-Za-z0-9._-], not hidden, not *.manifest.json or *.age; encrypted backups add .age)"
                        .into(),
                ));
            }
            Ok(Args::Snapshot {
                name: Some(name.clone()),
            })
        }
        _ => Err(usage()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Result<Args, Error> {
        parse(&text.split(' ').map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn a_snapshot_takes_an_optional_plain_file_name() {
        assert_eq!(parse(&[]).unwrap(), Args::Snapshot { name: None });
        assert_eq!(
            args("--name kanade-20261003T070900Z-pre-98c2b31.sqlite").unwrap(),
            Args::Snapshot {
                name: Some("kanade-20261003T070900Z-pre-98c2b31.sqlite".into())
            }
        );
        for bad in [
            "--name",
            "--name ../x.sqlite",
            "--name a/b.sqlite",
            "--name .hidden.sqlite",
            "--name x.sqlite.manifest.json",
            "--name x.sqlite.age",
            "--name x y",
            "--other",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
        assert!(parse(&["--name".into(), String::new()]).is_err());
    }

    #[test]
    fn the_age_tools_take_their_flags_in_any_order() {
        assert_eq!(
            args("keygen --out /keys/id").unwrap(),
            Args::Keygen {
                out: "/keys/id".into()
            }
        );
        assert_eq!(args("encrypt").unwrap(), Args::Encrypt { recipients: None });
        assert_eq!(
            args("encrypt --recipients /run/secrets/r").unwrap(),
            Args::Encrypt {
                recipients: Some("/run/secrets/r".into())
            }
        );
        assert_eq!(
            args("decrypt --out y --identity k --in x.age").unwrap(),
            Args::Decrypt {
                identity: "k".into(),
                input: "x.age".into(),
                output: "y".into(),
            }
        );
        for bad in [
            "keygen",
            "keygen --out",
            "keygen --out a --out b",
            "encrypt --recipients",
            "encrypt --identity k",
            "decrypt --identity k --in x.age",
            "decrypt --identity k --in x.age --out y extra",
            "decrypt --identity k --identity k --in x --out y",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
        assert!(parse(&["keygen".into(), "--out".into(), String::new()]).is_err());
    }
}
