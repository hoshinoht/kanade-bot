pub mod healthcheck;

use crate::runtime::error::Error;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve { offline: bool },
    Healthcheck { url: Option<String> },
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
        "ctl" if arguments.len() == 1 => Ok(Command::Reserved { name: "ctl" }),
        "import" if arguments.len() == 1 => Ok(Command::Reserved { name: "import" }),
        "export" if arguments.len() == 1 => Ok(Command::Reserved { name: "export" }),
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
    Error::Usage("usage: kanade {serve [--offline]|healthcheck [--url http://127.0.0.1:8080/healthz]|ctl|import|export}".into())
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
}
