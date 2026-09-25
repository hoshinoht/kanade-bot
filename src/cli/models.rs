//! `kanade models check [--probe]`: lists the gateway catalog and the role
//! routes from the model environment (aliases and reasoning seeds; the store
//! belongs to serve and is not read); `--probe` sends one tiny completion per
//! configured role. Live gateway calls; the key is never printed.

use std::{collections::BTreeMap, io::Write, sync::Arc};

use crate::{
    domain::settings::Models as StoredModels,
    infrastructure::llm::{
        TrustZone,
        governor::XorShift,
        identity::Passthrough,
        setup::{
            CatalogModel, Listing, ModelRoles, ModelSetup, Models, PROBE_TIMEOUT, ProbeOutcome,
            build_with_groups,
        },
    },
    runtime::{config::ModelSettings, error::Error},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub probe: bool,
}

pub fn parse(arguments: &[String]) -> Result<Args, Error> {
    match arguments {
        [check] if check == "check" => Ok(Args { probe: false }),
        [check, probe] if check == "check" && probe == "--probe" => Ok(Args { probe: true }),
        _ => Err(Error::Usage("usage: kanade models check [--probe]".into())),
    }
}

pub async fn run(args: Args, environment: &BTreeMap<String, String>) -> Result<(), Error> {
    check(args, environment, &mut std::io::stdout()).await
}

/// Writes the report to `out`; fails when the listing or any probe fails.
pub async fn check(
    args: Args,
    environment: &BTreeMap<String, String>,
    out: &mut impl Write,
) -> Result<(), Error> {
    let settings = ModelSettings::from_mapping(environment)?;
    let Some(base_url) = settings.base_url.clone() else {
        return Err(Error::Configuration(
            "KANADE_MODEL_BASE_URL is not set; models are disabled".into(),
        ));
    };
    let key = settings
        .read_key()?
        .map(|key| key.expose().as_bytes().to_vec());
    // The store is owned by serve, so roles come from the env seeds alone:
    // what serve runs for any role without a saved row.
    let mut stored = StoredModels::default();
    for (slot, alias, reasoning) in [
        (
            &mut stored.extraction,
            &settings.extract_model,
            settings.extract_reasoning,
        ),
        (
            &mut stored.chat,
            &settings.chat_model,
            settings.chat_reasoning,
        ),
        (
            &mut stored.rewrite,
            &settings.rewrite_model,
            settings.rewrite_reasoning,
        ),
    ] {
        slot.alias.clone_from(alias);
        if let Some(level) = reasoning {
            slot.reasoning = level;
        }
    }
    let setup = ModelSetup {
        base_url: Some(base_url.clone()),
        roles: ModelRoles::from(&stored),
        key,
        ca_file: settings.ca_file.clone(),
        permits: u32::from(settings.permits),
        allow_external_unmasked: settings.allow_external_unmasked,
    };
    let random = Arc::new(XorShift::new(uuid::Uuid::new_v4().as_u64_pair().0));
    let stack = match build_with_groups(setup, &settings.groups, random) {
        Ok(Models::Ready(stack)) => stack,
        Ok(Models::Unavailable) => return Err(Error::Configuration("models are disabled".into())),
        Err(error) => return Err(Error::Configuration(format!("model setup: {error}"))),
    };
    let io = |_| Error::Unavailable("could not write the report".into());
    writeln!(
        out,
        "gateway: {base_url} (key: {}, roots: {})",
        if settings.key_file.is_some() {
            "set"
        } else {
            "none"
        },
        if settings.ca_file.is_some() {
            "CA file"
        } else {
            "webpki"
        }
    )
    .map_err(io)?;

    let report = stack.check_startup().await;
    let mut failures = Vec::new();
    match &report.listing {
        Listing::Listed { models } => writeln!(out, "catalog: {models} models").map_err(io)?,
        Listing::Degraded { reason_code } => {
            writeln!(out, "catalog: unavailable ({reason_code})").map_err(io)?;
            failures.push(format!("listing failed ({reason_code})"));
        }
    }
    let catalog = stack.catalog();
    // Variants (`<base>:<level>`) are listed under their base, as the picker does.
    for model in &catalog.models {
        if catalog.variant(&model.alias).is_some() {
            continue;
        }
        let variants: Vec<&str> = catalog
            .models
            .iter()
            .filter(|other| {
                catalog
                    .variant(&other.alias)
                    .is_some_and(|variant| variant.base == model.alias)
            })
            .filter_map(|other| other.alias.rsplit_once(':').map(|(_, level)| level))
            .collect();
        let mut line = describe(model);
        if !variants.is_empty() {
            line.push_str(&format!(" variants: {}", variants.join(", ")));
        }
        writeln!(out, "  {line}").map_err(io)?;
    }

    writeln!(out, "roles (env seeds; saved settings are not read):").map_err(io)?;
    let efforts = stack.efforts();
    for role in ModelRoles::ALL {
        let Some(route) = stack.governor.route(role) else {
            writeln!(out, "  {} (not configured)", role.as_str()).map_err(io)?;
            continue;
        };
        let listed = catalog
            .models
            .iter()
            .any(|model| model.alias == route.alias);
        let fixed = catalog.variant(&route.alias).is_some();
        let effort = efforts
            .get(&role)
            .map_or("off".to_owned(), |status| match status.stranded {
                None if fixed => format!("{} (fixed)", status.effort.as_str()),
                Some(configured) => format!(
                    "{} (configured {})",
                    status.effort.as_str(),
                    configured.as_str()
                ),
                None => status.effort.as_str().to_owned(),
            });
        let trust = match (route.external, route.unmasked_allowed) {
            (false, _) => "homelab",
            (true, true) => "external UNMASKED",
            (true, false) => "external refused",
        };
        writeln!(
            out,
            "  {} {}{} effort={effort} route={trust}",
            role.as_str(),
            route.alias,
            if listed || !catalog.listed {
                ""
            } else {
                " (not listed)"
            },
        )
        .map_err(io)?;
    }
    for warning in &report.warnings {
        writeln!(out, "warning: {warning}").map_err(io)?;
    }

    if args.probe {
        writeln!(out, "probe:").map_err(io)?;
        for role in ModelRoles::ALL {
            let Some(result) = stack.probe(role, &Passthrough, PROBE_TIMEOUT).await else {
                continue;
            };
            let head = format!(
                "  {} {} effort={}",
                role.as_str(),
                result.alias,
                result.effort.as_str()
            );
            match &result.outcome {
                ProbeOutcome::Ok {
                    latency_ms,
                    finish_reason,
                } => writeln!(out, "{head} ok {latency_ms} ms finish={finish_reason}"),
                ProbeOutcome::Refused(reason) => writeln!(out, "{head} refused: {reason}"),
                ProbeOutcome::Failed(reason) => writeln!(out, "{head} failed: {reason}"),
            }
            .map_err(io)?;
            if !result.outcome.is_ok() {
                failures.push(format!("{} probe failed", role.as_str()));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Unavailable(format!(
            "models check failed: {}",
            failures.join("; ")
        )))
    }
}

fn describe(model: &CatalogModel) -> String {
    let zone = model.trust_zone.map_or("unknown", |zone| match zone {
        TrustZone::Local => "local",
        TrustZone::PrivateNetwork => "private_network",
        TrustZone::External => "external",
    });
    let efforts = match (&model.reasoning_efforts, model.reasoning_control) {
        (_, false) => "unsupported".to_owned(),
        (None, true) => "any".to_owned(),
        (Some(levels), true) => levels
            .iter()
            .map(|level| level.as_str())
            .collect::<Vec<_>>()
            .join(","),
    };
    let efforts = if model.off_allowed() {
        efforts
    } else {
        format!("{efforts} (off not allowed)")
    };
    let mut line = format!(
        "{} zone={zone} homelab={} efforts={efforts}",
        model.alias,
        if model.leaves_homelab {
            "leaves"
        } else {
            "stays"
        },
    );
    if let Some(tokens) = model.context_tokens {
        line.push_str(&format!(" context={tokens}"));
    }
    if let Some(admission) = model.admission {
        line.push_str(&format!(" in_flight={}", admission.concurrency()));
    }
    line
}
