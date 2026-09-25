use std::{collections::BTreeMap, future::Future, pin::Pin};

use serde::Serialize;

use crate::{
    api::server,
    cli::{self, Command, healthcheck},
};

use super::{
    config::{HealthcheckConfig, RuntimeConfig, ServeConfig},
    error::Error,
    serve,
};

pub async fn run(
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
) -> Result<(), Error> {
    match cli::parse(arguments)? {
        Command::Serve { offline: true } => {
            server::serve_offline(RuntimeConfig::from_mapping(&environment)?).await
        }
        Command::Serve { offline: false } => {
            serve::run(ServeConfig::from_mapping(&environment)?).await
        }
        Command::Healthcheck { url } => {
            healthcheck::check(HealthcheckConfig::from_mapping(
                &environment,
                url.as_deref(),
            )?)
            .await
        }
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
        }
    }
}
