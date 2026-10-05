pub mod healthcheck;
pub mod models;

use std::path::PathBuf;

use chrono::NaiveDate;

use crate::runtime::error::Error;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve { offline: bool },
    Healthcheck { url: Option<String> },
    ImportV4(ImportV4Args),
    Backup(BackupArgs),
    Models(models::Args),
    Reserved { name: &'static str },
}

pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, Error> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(usage());
    };
    match command {
        "serve" => parse_serve(&arguments[1..]),
        "healthcheck" => parse_healthcheck(&arguments[1..]),
        "models" => models::parse(&arguments[1..]).map(Command::Models),
        "ctl" if arguments.len() == 1 => Ok(Command::Reserved { name: "ctl" }),
        "import" => parse_import(&arguments[1..]),
        "backup" => parse_backup(&arguments[1..]),
        _ => Err(usage()),
    }
}

fn parse_serve(arguments: &[String]) -> Result<Command, Error> {
    match arguments {
        [] => Ok(Command::Serve { offline: false }),
        [offline] if offline == "--offline" => Ok(Command::Serve { offline: true }),
        _ => Err(usage()),
    }
}

fn parse_healthcheck(arguments: &[String]) -> Result<Command, Error> {
    match arguments {
        [] => Ok(Command::Healthcheck { url: None }),
        [flag, url] if flag == "--url" => Ok(Command::Healthcheck {
            url: Some(url.clone()),
        }),
        _ => Err(usage()),
    }
}

/// `import v4 --from <snapshot> [--since YYYY-MM-DD] [--refresh-logs]
/// [--apply]`; a dry run unless `--apply`.
#[derive(Debug, PartialEq, Eq)]
pub struct ImportV4Args {
    pub from: PathBuf,
    pub since: Option<NaiveDate>,
    pub apply: bool,
    pub refresh_logs: bool,
}

fn parse_import(arguments: &[String]) -> Result<Command, Error> {
    let Some((source, mut rest)) = arguments.split_first() else {
        return Err(usage());
    };
    if source != "v4" {
        return Err(usage());
    }
    let (mut from, mut since, mut apply, mut refresh_logs) = (None, None, false, false);
    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        match flag.as_str() {
            "--apply" if !apply => apply = true,
            "--refresh-logs" if !refresh_logs => refresh_logs = true,
            "--from" | "--since" => {
                let Some((value, tail)) = rest.split_first() else {
                    return Err(usage());
                };
                rest = tail;
                if flag == "--from" && from.is_none() && !value.is_empty() {
                    from = Some(PathBuf::from(value));
                } else if flag == "--since" && since.is_none() {
                    since = Some(
                        (value.len() == 10)
                            .then(|| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
                            .flatten()
                            .ok_or_else(|| Error::Usage("--since must be YYYY-MM-DD".into()))?,
                    );
                } else {
                    return Err(usage());
                }
            }
            _ => return Err(usage()),
        }
    }
    let from = from.ok_or_else(usage)?;
    Ok(Command::ImportV4(ImportV4Args {
        from,
        since,
        apply,
        refresh_logs,
    }))
}

/// `backup [--name FILE]`: a snapshot named `FILE` (a plain file name) in
/// `KANADE_BACKUP_DIR`, or a timestamped default name.
#[derive(Debug, PartialEq, Eq)]
pub struct BackupArgs {
    pub name: Option<String>,
}

/// The longest accepted `--name`, leaving room for `.manifest.json`.
const MAX_BACKUP_NAME: usize = 200;

fn parse_backup(arguments: &[String]) -> Result<Command, Error> {
    match arguments {
        [] => Ok(Command::Backup(BackupArgs { name: None })),
        [flag, name] if flag == "--name" => {
            let plain = !name.is_empty()
                && name.len() <= MAX_BACKUP_NAME
                && !name.starts_with('.')
                && !name.ends_with(".manifest.json")
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte));
            if !plain {
                return Err(Error::Usage(
                    "--name must be a plain file name ([A-Za-z0-9._-], not hidden, not *.manifest.json)"
                        .into(),
                ));
            }
            Ok(Command::Backup(BackupArgs {
                name: Some(name.clone()),
            }))
        }
        _ => Err(usage()),
    }
}

fn usage() -> Error {
    Error::Usage("usage: kanade {serve [--offline]|healthcheck [--url http://127.0.0.1:8080/healthz]|models check [--probe]|import v4 --from PATH [--since YYYY-MM-DD] [--refresh-logs] [--apply]|backup [--name FILE]|ctl}".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_offline_serve_is_accepted_as_a_fixture_mode() {
        assert_eq!(
            parse(["serve".into(), "--offline".into()]).unwrap(),
            Command::Serve { offline: true }
        );
        assert!(parse(["serve".into(), "--other".into()]).is_err());
    }

    #[test]
    fn ctl_stays_reserved_and_export_is_gone() {
        assert_eq!(
            parse(["ctl".into()]).unwrap(),
            Command::Reserved { name: "ctl" }
        );
        assert!(parse(["export".into()]).is_err());
    }

    fn args(text: &str) -> Result<Command, Error> {
        parse(text.split(' ').map(str::to_owned))
    }

    #[test]
    fn backup_takes_an_optional_plain_file_name() {
        assert_eq!(
            args("backup").unwrap(),
            Command::Backup(BackupArgs { name: None })
        );
        assert_eq!(
            args("backup --name kanade-20261003T070900Z-pre-98c2b31.sqlite").unwrap(),
            Command::Backup(BackupArgs {
                name: Some("kanade-20261003T070900Z-pre-98c2b31.sqlite".into())
            })
        );
        for bad in [
            "backup --name",
            "backup --name ../x.sqlite",
            "backup --name a/b.sqlite",
            "backup --name .hidden.sqlite",
            "backup --name x.sqlite.manifest.json",
            "backup --name x y",
            "backup --other",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
        assert!(parse(["backup".into(), "--name".into(), String::new()]).is_err());
    }

    #[test]
    fn import_v4_is_a_dry_run_unless_applied() {
        assert_eq!(
            args("import v4 --from /import/v4.sqlite").unwrap(),
            Command::ImportV4(ImportV4Args {
                from: PathBuf::from("/import/v4.sqlite"),
                since: None,
                apply: false,
                refresh_logs: false,
            })
        );
        assert_eq!(
            args("import v4 --apply --since 2026-09-01 --from snap.db").unwrap(),
            Command::ImportV4(ImportV4Args {
                from: PathBuf::from("snap.db"),
                since: NaiveDate::from_ymd_opt(2026, 9, 1),
                apply: true,
                refresh_logs: false,
            })
        );
        assert_eq!(
            args("import v4 --from snap.db --refresh-logs").unwrap(),
            Command::ImportV4(ImportV4Args {
                from: PathBuf::from("snap.db"),
                since: None,
                apply: false,
                refresh_logs: true,
            })
        );
        for bad in [
            "import",
            "import v3 --from a",
            "import v4",
            "import v4 --from",
            "import v4 --from a --from b",
            "import v4 --from a --apply --apply",
            "import v4 --from a --refresh-logs --refresh-logs",
            "import v4 --from a --since 2026-9-1",
            "import v4 --from a --since 2026-02-30",
            "import v4 --from a --other",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }
}
