mod cases;
mod probe;
mod report;
mod scoring;

#[cfg(all(test, feature = "test-support"))]
mod tests;

use std::{env, ffi::OsString, path::PathBuf, process::ExitCode, sync::Arc};

use kanade::infrastructure::llm::{
    Effort,
    governor::{Random, Role},
    identity::{
        BotIdentity, CodeLexicon, NamePool, PseudonymCodec, PseudonymConfig, ScanExemptions,
        SystemRng,
    },
    setup::{ModelRoles, ModelSetup, Models, RoleEffort, RoleModel, build},
};

const USAGE: &str = "Usage: cargo run --locked --example privacy_eval -- --live --base-url <endpoint> --alias <model-alias> --key-file <path>";

struct LiveOptions {
    base_url: String,
    alias: String,
    key_file: PathBuf,
}

fn parse_options(args: impl IntoIterator<Item = OsString>) -> Result<Option<LiveOptions>, ()> {
    let mut args = args.into_iter();
    let mut live = false;
    let mut help = false;
    let mut base_url = None;
    let mut alias = None;
    let mut key_file = None;
    while let Some(argument) = args.next() {
        let argument = argument.into_string().map_err(|_| ())?;
        match argument.as_str() {
            "--live" if !live => live = true,
            "--help" | "-h" => help = true,
            "--base-url" if base_url.is_none() => {
                base_url = Some(args.next().ok_or(())?.into_string().map_err(|_| ())?);
            }
            "--alias" if alias.is_none() => {
                alias = Some(args.next().ok_or(())?.into_string().map_err(|_| ())?);
            }
            "--key-file" if key_file.is_none() => {
                key_file = Some(PathBuf::from(args.next().ok_or(())?));
            }
            _ => return Err(()),
        }
    }
    if help {
        return Ok(None);
    }
    if !live {
        return Err(());
    }
    let base_url = base_url
        .filter(|value| !value.trim().is_empty())
        .ok_or(())?;
    let alias = alias.filter(|value| !value.trim().is_empty()).ok_or(())?;
    let key_file = key_file.ok_or(())?;
    Ok(Some(LiveOptions {
        base_url,
        alias,
        key_file,
    }))
}

fn model_setup(base_url: String, alias: String, key: Vec<u8>) -> ModelSetup {
    ModelSetup {
        base_url: Some(base_url),
        key: Some(key),
        ca_file: None,
        roles: ModelRoles {
            extraction: RoleModel {
                alias: None,
                effort: RoleEffort::Level(Effort::Off),
            },
            chat: RoleModel::default(),
            rewrite: RoleModel {
                alias: Some(alias),
                effort: RoleEffort::Level(Effort::Off),
            },
        },
        permits: 1,
        allow_external_unmasked: false,
        pseudonymize: true,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let options = match parse_options(env::args_os().skip(1)) {
        Ok(None) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Some(options)) => options,
        Err(()) => {
            eprintln!("A live probe requires --live, --base-url, --alias, and --key-file.");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    match run_live(options).await {
        Ok(report) => {
            let failed = report.failed_acceptance();
            match serde_json::to_string(&report) {
                Ok(json) => println!("{json}"),
                Err(_) => {
                    eprintln!("Could not serialize the aggregate privacy report.");
                    return ExitCode::FAILURE;
                }
            }
            if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(()) => {
            eprintln!("The privacy probe could not start; no prompt or reply was emitted.");
            ExitCode::FAILURE
        }
    }
}

async fn run_live(options: LiveOptions) -> Result<report::AggregateReport, ()> {
    let evaluation = cases::load()?;
    if evaluation.cases.is_empty() || evaluation.cases.len() > probe::MAX_CASES {
        return Err(());
    }
    let key = kanade::runtime::secrets::read_secret(&options.key_file, "--key-file")
        .map_err(|_| ())?
        .into_bytes();
    kanade::runtime::tls::install_ring_provider().map_err(|_| ())?;
    let random: Arc<dyn Random> = Arc::new(SystemRng::new());
    let setup = model_setup(options.base_url, options.alias, key);
    let Models::Ready(stack) = build(setup, Arc::clone(&random)).map_err(|_| ())? else {
        return Err(());
    };
    let route = stack.governor.route(Role::Rewrite).ok_or(())?;
    if !route.external || route.unmasked_allowed || !stack.client.masking() {
        return Err(());
    }

    let schema = probe::output_schema();
    let scan_exemptions = ScanExemptions::default()
        .with_texts([probe::AUDIT_PROMPT, probe::TASK_PROMPT, "propose_add"])
        .with_value(&schema.schema);
    let codec = PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin().with_terms([evaluation.boss_alias_collision.as_str()]),
        bot: BotIdentity::default(),
        extra_exclusions: Vec::new(),
        random,
    })
    .with_scan_exemptions(&scan_exemptions);

    let candidate_count = scoring::candidate_count(
        evaluation
            .cases
            .iter()
            .flat_map(|case| case.candidates().iter().map(String::as_str)),
    );
    let mut report = report::AggregateReport::new(evaluation.cases.len(), candidate_count);
    for (index, case) in evaluation.cases.iter().enumerate() {
        probe::pace_before_case(index).await;
        match probe::run_case(&stack.client, &route, &codec, case).await {
            Ok(observation) => report.record_success(observation),
            Err(()) => report.record_failure(),
        }
    }
    Ok(report)
}
