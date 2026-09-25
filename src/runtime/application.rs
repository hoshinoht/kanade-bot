use std::{collections::BTreeMap, future::Future, pin::Pin};

use serde::Serialize;

use crate::{
    api::server,
    cli::{self, Command, healthcheck},
    import,
};

use super::{
    config::{self, HealthcheckConfig, ImportConfig, RuntimeConfig, ServeConfig},
    error::Error,
    serve,
};

pub async fn run(
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
) -> Result<(), Error> {
    let command = cli::parse(arguments)?;
    let resolved = config::resolve(environment)?;
    dispatch(command, &resolved.values)
        .await
        .map_err(|error| resolved.annotate(error))
}

async fn dispatch(command: Command, environment: &BTreeMap<String, String>) -> Result<(), Error> {
    match command {
        Command::Serve { offline: true } => {
            server::serve_offline(RuntimeConfig::from_mapping(environment)?).await
        }
        Command::Serve { offline: false } => {
            serve::run(ServeConfig::from_mapping(environment)?).await
        }
        Command::Healthcheck { url } => {
            healthcheck::check(HealthcheckConfig::from_mapping(
                environment,
                url.as_deref(),
            )?)
            .await
        }
        Command::ImportV4(args) => {
            let config = ImportConfig::from_mapping(environment)?;
            let options = import::v4::Options {
                from: args.from,
                since: args.since,
                apply: args.apply,
                refresh_logs: args.refresh_logs,
            };
            let report = import::v4::run(&options, &config, import::v4::system_now())
                .await
                .map_err(|error| match error {
                    import::v4::ImportError::Store(_) => {
                        Error::Unavailable(format!("import v4: {error}"))
                    }
                    _ => Error::Configuration(format!("import v4: {error}")),
                })?;
            print!("{report}");
            Ok(())
        }
        Command::Models(args) => cli::models::run(args, environment).await,
        Command::Reserved { name } => Err(Error::Unavailable(format!(
            "{name} is not implemented in the runtime bootstrap"
        ))),
    }
}

#[derive(Clone, Copy)]
pub struct OfflineApplication;

/// The `/healthz` document. `status` is `ok` only when the process can do
/// its job; anything else answers 503.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Health {
    pub status: &'static str,
    /// `offline` or `live`.
    pub mode: &'static str,
    pub scheduler: &'static str,
    pub storage: &'static str,
    pub discord: &'static str,
    /// Live gateway only: events refused for another guild or no guild.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_events: Option<DroppedHealth>,
    /// Live tick only: seconds since the last completed tick.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tick_age_seconds: Option<u64>,
    /// Live serve only: `disabled`, `idle`, `busy` or `degraded`. Never
    /// decides `status`: chat is not essential to the process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat: Option<&'static str>,
    /// Live gateway only: `disabled`, `idle`, `running` or `degraded`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extraction: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DroppedHealth {
    pub other_guild: u64,
    pub no_guild: u64,
}

impl Health {
    pub fn is_ok(&self) -> bool {
        self.status == "ok"
    }
}

pub type HealthFuture<'a> = Pin<Box<dyn Future<Output = Health> + Send + 'a>>;

/// Live health, probed per request.
pub trait HealthProbe: Send + Sync + std::fmt::Debug {
    fn health(&self) -> HealthFuture<'_>;
}

impl OfflineApplication {
    pub fn health(self) -> Health {
        Health {
            status: "ok",
            mode: "offline",
            scheduler: "unavailable",
            storage: "unavailable",
            discord: "unavailable",
            dropped_events: None,
            last_tick_age_seconds: None,
            chat: None,
            extraction: None,
        }
    }
}
