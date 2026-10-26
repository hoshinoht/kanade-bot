//! Serve's side of Setup's saved secrets: the desk over the saveable secret
//! files, the live checks (a candidate bot token against Discord, a
//! candidate model key against the configured gateway) and "Restart now".

use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};

use twilight_model::id::Id;

use crate::{
    api::{
        dto::setup::{PrivilegedIntent, SecretCheck, SecretCheckResult as Result, SecretName},
        state::{RestartPort, SecretChecks, SecretDesk},
    },
    discord::transport::{DiscordTransport, Outcome, TransportConfig, TwilightTransport},
    infrastructure::llm::setup::{KeyCheck, check_key},
    runtime::{config::ServeConfig, error::Error},
};

use super::restart;

/// The desk Setup saves into; `None` without a saved directory.
pub(super) fn desk(config: &ServeConfig) -> std::result::Result<Option<Arc<SecretDesk>>, Error> {
    if config.saveable_secrets.is_empty() {
        return Ok(None);
    }
    let checks = LiveChecks {
        model_base_url: config.models.base_url.clone(),
        model_ca: config.models.ca_file()?,
    };
    let restart = config.supervised.then(|| {
        Arc::new(|| {
            restart::preflight().map_err(|error| error.to_string())?;
            restart::request();
            Ok(())
        }) as RestartPort
    });
    Ok(Some(Arc::new(SecretDesk::new(
        config.saveable_secrets.clone(),
        Arc::new(checks),
        restart,
    ))))
}

struct LiveChecks {
    model_base_url: Option<String>,
    model_ca: Option<PathBuf>,
}

impl SecretChecks for LiveChecks {
    fn check<'a>(
        &'a self,
        name: SecretName,
        value: &'a str,
    ) -> Pin<Box<dyn Future<Output = SecretCheck> + Send + 'a>> {
        Box::pin(async move {
            match name {
                SecretName::DiscordToken => discord(value.trim()).await,
                SecretName::ModelApiKey => match &self.model_base_url {
                    Some(base) => model(base, self.model_ca.as_deref(), value.trim()).await,
                    None => SecretCheck::new(Result::NotChecked),
                },
                _ => SecretCheck::new(Result::NotChecked),
            }
        })
    }
}

/// The bot's own user and its application's intents, through a throwaway
/// transport (no gateway; `/applications/@me` names no application, so the
/// placeholder id is never sent).
async fn discord(token: &str) -> SecretCheck {
    if token.is_empty() {
        return SecretCheck::new(Result::Refused);
    }
    let transport =
        TwilightTransport::new(token.to_owned(), Id::new(1), TransportConfig::default());
    let user = match transport.current_user().await {
        Outcome::Delivered(user) => user,
        Outcome::DefinitelyRejected(_) => return SecretCheck::new(Result::Refused),
        Outcome::Ambiguous(_) => return SecretCheck::new(Result::Unreachable),
    };
    let missing = match transport.application_info().await {
        Outcome::Delivered(application) => Some(PrivilegedIntent::missing(application.flags)),
        _ => None,
    };
    SecretCheck {
        bot_name: Some(user.name),
        missing_intents: missing,
        ..SecretCheck::new(Result::Ok)
    }
}

async fn model(base: &str, ca: Option<&std::path::Path>, key: &str) -> SecretCheck {
    match check_key(base, ca, key).await {
        KeyCheck::Accepted(models) => SecretCheck {
            models: Some(u32::try_from(models).unwrap_or(u32::MAX)),
            ..SecretCheck::new(Result::Ok)
        },
        KeyCheck::Refused => SecretCheck::new(Result::Refused),
        KeyCheck::Unreachable => SecretCheck::new(Result::Unreachable),
    }
}
