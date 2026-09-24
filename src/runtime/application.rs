use std::collections::BTreeMap;

use serde::Serialize;

use crate::{
    api::server,
    cli::{self, Command, healthcheck},
};

use super::{
    config::{HealthcheckConfig, RuntimeConfig},
    error::Error,
};

pub async fn run(
    arguments: Vec<String>,
    environment: BTreeMap<String, String>,
) -> Result<(), Error> {
    match cli::parse(arguments)? {
        Command::Serve { offline: true } => {
            server::serve_offline(RuntimeConfig::from_mapping(&environment)?).await
        }
        Command::Serve { offline: false } => Err(Error::Unavailable(
            "production adapters are unavailable; use `serve --offline` only for local development"
                .into(),
        )),
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

#[derive(Serialize)]
pub struct Health {
    status: &'static str,
    mode: &'static str,
    scheduler: &'static str,
    storage: &'static str,
    discord: &'static str,
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
