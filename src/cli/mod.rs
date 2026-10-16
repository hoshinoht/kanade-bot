pub mod admin;
pub mod backup;
pub mod emojis;
pub mod healthcheck;
pub mod models;
pub mod restore;
pub mod token;

use crate::runtime::{error::Error, restore::Options as RestoreOptions};

/// What `kanade --version` prints: the crate version, the one release number.
pub const VERSION: &str = concat!("kanade ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve {
        offline: bool,
    },
    Healthcheck {
        url: Option<String>,
    },
    Backup(backup::Args),
    Restore(RestoreOptions),
    /// `portal …` and `sessions …`: online, through the admin API.
    Admin(admin::Args),
    TokenRotate(token::Args),
    Models(models::Args),
    CtlEmojis(emojis::Args),
    /// `--version`/`-V`: needs no configuration.
    Version,
}

pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Command, Error> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let Some(command) = arguments.first().map(String::as_str) else {
        return Err(usage());
    };
    match command {
        "--version" | "-V" if arguments.len() == 1 => Ok(Command::Version),
        "serve" => parse_serve(&arguments[1..]),
        "healthcheck" => parse_healthcheck(&arguments[1..]),
        "models" => models::parse(&arguments[1..]).map(Command::Models),
        "ctl" => match arguments.get(1).map(String::as_str) {
            Some("emojis") => emojis::parse(&arguments[2..]).map(Command::CtlEmojis),
            _ => Err(usage()),
        },
        "backup" => backup::parse(&arguments[1..]).map(Command::Backup),
        "restore" => restore::parse(&arguments[1..]).map(Command::Restore),
        "portal" => admin::parse_portal(&arguments[1..]).map(Command::Admin),
        "sessions" => admin::parse_sessions(&arguments[1..]).map(Command::Admin),
        "token" => token::parse(&arguments[1..]).map(Command::TokenRotate),
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

fn usage() -> Error {
    Error::Usage(format!(
        "usage: kanade {{serve [--offline]|healthcheck [--url http://127.0.0.1:8080/healthz]|models check [--probe]|{}|{}|{}|{}|ctl emojis [--dry-run] [--dir PATH]|--version}}",
        backup::USAGE,
        restore::USAGE,
        admin::USAGE,
        token::USAGE
    ))
}

/// `--switch` and `--flag value` arguments in any order, each at most once
/// and every value non-empty; anything else is the command's `usage`.
fn options<'a, const S: usize, const V: usize>(
    arguments: &'a [String],
    switches: [&str; S],
    valued: [&str; V],
    usage: fn() -> Error,
) -> Result<([bool; S], [Option<&'a str>; V]), Error> {
    let (mut on, mut values) = ([false; S], [None; V]);
    let mut rest = arguments;
    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        if let Some(slot) = switches.iter().position(|name| name == flag) {
            if std::mem::replace(&mut on[slot], true) {
                return Err(usage());
            }
        } else if let Some(slot) = valued.iter().position(|name| name == flag) {
            let Some((value, tail)) = rest.split_first().filter(|(value, _)| !value.is_empty())
            else {
                return Err(usage());
            };
            rest = tail;
            if values[slot].replace(value.as_str()).is_some() {
                return Err(usage());
            }
        } else {
            return Err(usage());
        }
    }
    Ok((on, values))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn only_offline_serve_is_accepted_as_a_fixture_mode() {
        assert_eq!(
            parse(["serve".into(), "--offline".into()]).unwrap(),
            Command::Serve { offline: true }
        );
        assert!(parse(["serve".into(), "--other".into()]).is_err());
    }

    #[test]
    fn version_is_a_sole_flag() {
        for flag in ["--version", "-V"] {
            assert_eq!(parse([flag.into()]).unwrap(), Command::Version);
            assert!(parse([flag.into(), "serve".into()]).is_err());
        }
    }

    #[test]
    fn ctl_takes_emojis_and_unknown_commands_get_usage() {
        assert_eq!(
            parse(["ctl".into(), "emojis".into(), "--dry-run".into()]).unwrap(),
            Command::CtlEmojis(emojis::Args {
                dry_run: true,
                dir: None,
            })
        );
        assert!(parse(["ctl".into()]).is_err());
        assert!(parse(["ctl".into(), "other".into()]).is_err());
        let usage = parse(["unknown".into()]).unwrap_err().to_string();
        for command in ["export", "import"] {
            assert_eq!(parse([command.into()]).unwrap_err().to_string(), usage);
        }
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
    fn operator_commands_route_to_their_parsers() {
        assert_eq!(
            args("restore --from /backups/k.sqlite --apply").unwrap(),
            Command::Restore(RestoreOptions {
                from: PathBuf::from("/backups/k.sqlite"),
                identity: None,
                apply: true,
            })
        );
        assert_eq!(
            args("portal status").unwrap(),
            Command::Admin(admin::Args {
                action: admin::Action::PortalStatus,
                url: None,
            })
        );
        assert_eq!(
            args("sessions list --url http://127.0.0.1:8080").unwrap(),
            Command::Admin(admin::Args {
                action: admin::Action::SessionsList,
                url: Some("http://127.0.0.1:8080".into()),
            })
        );
        assert_eq!(
            args("token rotate --out /s/admin_token").unwrap(),
            Command::TokenRotate(token::Args {
                out: PathBuf::from("/s/admin_token"),
                force: false,
            })
        );
        for bad in ["restore", "portal", "sessions", "token", "token rotate"] {
            assert!(args(bad).is_err(), "{bad}");
        }
        let text = parse(Vec::<String>::new()).unwrap_err().to_string();
        for usage in [restore::USAGE, admin::USAGE, token::USAGE] {
            assert!(text.contains(usage), "{text}");
        }
    }
}
