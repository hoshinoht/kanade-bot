//! `kanade restore --from SNAPSHOT [--identity FILE] [--apply]`: a dry run
//! unless `--apply` (runtime/restore.rs does the work).

use std::path::PathBuf;

use crate::runtime::{error::Error, restore::Options};

pub const USAGE: &str = "restore --from PATH [--identity FILE] [--apply]";

fn usage() -> Error {
    Error::Usage(format!("usage: kanade {USAGE}"))
}

pub fn parse(arguments: &[String]) -> Result<Options, Error> {
    let (mut from, mut identity, mut apply) = (None, None, false);
    let mut rest = arguments;
    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        match flag.as_str() {
            "--apply" if !apply => apply = true,
            "--from" | "--identity" => {
                let Some((value, tail)) = rest.split_first().filter(|(value, _)| !value.is_empty())
                else {
                    return Err(usage());
                };
                rest = tail;
                let slot = if flag == "--from" {
                    &mut from
                } else {
                    &mut identity
                };
                if slot.replace(PathBuf::from(value)).is_some() {
                    return Err(usage());
                }
            }
            _ => return Err(usage()),
        }
    }
    Ok(Options {
        from: from.ok_or_else(usage)?,
        identity,
        apply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Result<Options, Error> {
        parse(&text.split(' ').map(str::to_owned).collect::<Vec<_>>())
    }

    #[test]
    fn restore_is_a_dry_run_unless_applied() {
        assert_eq!(
            args("--from /backups/k.sqlite").unwrap(),
            Options {
                from: "/backups/k.sqlite".into(),
                identity: None,
                apply: false,
            }
        );
        assert_eq!(
            args("--apply --identity /run/identity --from k.sqlite.age").unwrap(),
            Options {
                from: "k.sqlite.age".into(),
                identity: Some("/run/identity".into()),
                apply: true,
            }
        );
        for bad in [
            "--apply",
            "--from",
            "--from a --from b",
            "--from a --identity",
            "--from a --identity k --identity k",
            "--from a --apply --apply",
            "--from a --other",
            "--from a extra",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
        assert!(parse(&[]).is_err());
        assert!(parse(&["--from".into(), String::new()]).is_err());
    }
}
