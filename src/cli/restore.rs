//! `kanade restore --from SNAPSHOT [--identity FILE] [--apply]`: a dry run
//! unless `--apply` (runtime/restore.rs does the work).

use std::path::PathBuf;

use crate::runtime::{error::Error, restore::Options};

pub const USAGE: &str = "restore --from PATH [--identity FILE] [--apply]";

fn usage() -> Error {
    Error::Usage(format!("usage: kanade {USAGE}"))
}

pub fn parse(arguments: &[String]) -> Result<Options, Error> {
    let ([apply], [from, identity]) =
        super::options(arguments, ["--apply"], ["--from", "--identity"], usage)?;
    Ok(Options {
        from: from.map(PathBuf::from).ok_or_else(usage)?,
        identity: identity.map(PathBuf::from),
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
