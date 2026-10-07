//! `kanade ctl emojis [--dry-run] [--dir PATH]`: lists the application's
//! emojis and uploads the difficulty pills missing by their fixed names
//! (`diff_n`, `diff_h`, `diff_c`, `diff_x`) from `PATH/<name>.png` (default
//! `assets/emojis`). Idempotent; never deletes or renames an emoji;
//! `--dry-run` only lists. Live Discord HTTP with the bot token from
//! `KANADE_DISCORD_TOKEN_FILE`; no gateway session.

use std::{collections::BTreeMap, io::Write, path::PathBuf};

use crate::{
    bot::{
        cards::emojis::{self, PillStep},
        transport::{DiscordTransport, Outcome, TransportConfig, TwilightTransport},
    },
    runtime::{config::DiscordSettings, error::Error},
};

pub const DEFAULT_DIR: &str = "assets/emojis";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub dry_run: bool,
    pub dir: PathBuf,
}

fn usage() -> Error {
    Error::Usage("usage: kanade ctl emojis [--dry-run] [--dir PATH]".into())
}

pub fn parse(arguments: &[String]) -> Result<Args, Error> {
    let (mut dry_run, mut dir) = (false, None);
    let mut rest = arguments;
    while let Some((flag, tail)) = rest.split_first() {
        rest = tail;
        match flag.as_str() {
            "--dry-run" if !dry_run => dry_run = true,
            "--dir" if dir.is_none() => {
                let Some((value, tail)) = rest.split_first() else {
                    return Err(usage());
                };
                if value.is_empty() {
                    return Err(usage());
                }
                rest = tail;
                dir = Some(PathBuf::from(value));
            }
            _ => return Err(usage()),
        }
    }
    Ok(Args {
        dry_run,
        dir: dir.unwrap_or_else(|| PathBuf::from(DEFAULT_DIR)),
    })
}

pub async fn run(args: &Args, environment: &BTreeMap<String, String>) -> Result<(), Error> {
    let token = DiscordSettings::from_mapping(environment)?.read_token()?;
    let transport = match TwilightTransport::for_current_application(
        token.expose().to_owned(),
        TransportConfig::default(),
    )
    .await
    {
        Outcome::Delivered(transport) => transport,
        failed => {
            return Err(Error::Unavailable(format!(
                "ctl emojis: the application could not be read ({})",
                failed.failure_label().unwrap_or_default()
            )));
        }
    };
    sync(&transport, args, &mut std::io::stdout()).await
}

/// Sync through `transport` and write what was done to `out`. Fails when
/// the list fails or a pill could not be uploaded; a dry run's missing
/// pills are not a failure.
pub async fn sync<T: DiscordTransport>(
    transport: &T,
    args: &Args,
    out: &mut impl Write,
) -> Result<(), Error> {
    let report = emojis::sync(transport, &args.dir, args.dry_run)
        .await
        .map_err(|label| Error::Unavailable(format!("ctl emojis: listing failed ({label})")))?;
    write!(out, "{report}").map_err(|_| Error::Unavailable("could not write the report".into()))?;
    let failed = report
        .pills
        .iter()
        .any(|(_, step)| matches!(step, PillStep::Unreadable(_) | PillStep::Failed(_)));
    if failed {
        return Err(Error::Unavailable(
            "ctl emojis: some pills were not uploaded".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Result<Args, Error> {
        parse(
            &text
                .split(' ')
                .filter(|word| !word.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn takes_an_optional_dry_run_and_directory() {
        assert_eq!(
            args("").unwrap(),
            Args {
                dry_run: false,
                dir: PathBuf::from(DEFAULT_DIR)
            }
        );
        assert_eq!(
            args("--dir /pills --dry-run").unwrap(),
            Args {
                dry_run: true,
                dir: PathBuf::from("/pills")
            }
        );
        for bad in [
            "--dir",
            "--dry-run --dry-run",
            "--dir a --dir b",
            "--delete",
            "diff_n",
        ] {
            assert!(args(bad).is_err(), "{bad}");
        }
    }
}
