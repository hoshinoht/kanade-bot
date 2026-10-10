pub mod backup;
pub mod emojis;
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
    Backup(backup::Args),
    Models(models::Args),
    CtlEmojis(emojis::Args),
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
        "ctl" => match arguments.get(1).map(String::as_str) {
            Some("emojis") => emojis::parse(&arguments[2..]).map(Command::CtlEmojis),
            _ => Err(usage()),
        },
        "import" => parse_import(&arguments[1..]),
        "backup" => backup::parse(&arguments[1..]).map(Command::Backup),
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

fn usage() -> Error {
    Error::Usage(format!(
        "usage: kanade {{serve [--offline]|healthcheck [--url http://127.0.0.1:8080/healthz]|models check [--probe]|import v4 --from PATH [--since YYYY-MM-DD] [--refresh-logs] [--apply]|{}|ctl emojis [--dry-run] [--dir PATH]}}",
        backup::USAGE
    ))
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
    fn ctl_takes_the_emojis_subcommand_and_export_is_gone() {
        assert_eq!(
            parse(["ctl".into(), "emojis".into(), "--dry-run".into()]).unwrap(),
            Command::CtlEmojis(emojis::Args {
                dry_run: true,
                dir: None,
            })
        );
        assert!(parse(["ctl".into()]).is_err());
        assert!(parse(["ctl".into(), "other".into()]).is_err());
        assert!(parse(["export".into()]).is_err());
    }

    fn args(text: &str) -> Result<Command, Error> {
        parse(text.split(' ').map(str::to_owned))
    }

    #[test]
    fn backup_routes_to_its_own_parser() {
        assert_eq!(
            args("backup").unwrap(),
            Command::Backup(backup::Args::Snapshot { name: None })
        );
        assert_eq!(
            args("backup encrypt").unwrap(),
            Command::Backup(backup::Args::Encrypt { recipients: None })
        );
        assert!(args("backup --other").is_err());
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
