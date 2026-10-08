//! V02 live quality harness (draft `docs/notes/evidence/v02-quality-set-draft-2026-10-07.md`).
//!
//! Drives the real v5 chat and extraction code (live serve's Discord side over
//! the fake transport) with the real model client against wholly invented
//! fixtures, and writes one JSON per attempt plus a scored `summary.md`.
//! Explicit only: never part of the test suite or CI.
//!
//! Live:   KANADE_MODEL_BASE_URL=… KANADE_MODEL_KEY_FILE=… [KANADE_MODEL_CA_FILE=…]
//!         [KANADE_MODEL_GROUPS=…] [KANADE_MODEL_PERMITS=…] [KANADE_MODEL_CONTEXT=…]
//!         cargo run --features test-support --example v02_quality --
//!         --chat-alias A --chat-reasoning R --extract-alias A --extract-reasoning R
//! Offline: … --example v02_quality -- --dry-run [--cases C01,E07] [--cap N]
//! Rescore: … --example v02_quality -- --rescore <run-dir>

mod attempt;
mod cases;
mod collect;
mod dry;
mod output;
mod persona;
mod report;
mod requests;
mod score;
mod world;

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};

use chrono::{DateTime, Datelike, Timelike, Utc};
use kanade::{
    infrastructure::llm::governor::Role,
    runtime::{
        serve::{harness, settings},
        tls,
    },
};
use serde_json::{Value, json};

use attempt::{Shared, TempDir};
use kanade::{
    chat::persona::SelectionSource, infrastructure::files::load_personas,
    runtime::config::ServeConfig,
};
use persona::PersonaBundle;
use requests::{HARD_CAP, Requests};
use score::Detector;

/// The serve environment keys passed through from the process environment;
/// `KANADE_MODEL_CONTEXT` mirrors the deployed context windows and reserves.
const MODEL_ENV: [&str; 6] = [
    "KANADE_MODEL_BASE_URL",
    "KANADE_MODEL_KEY_FILE",
    "KANADE_MODEL_CA_FILE",
    "KANADE_MODEL_GROUPS",
    "KANADE_MODEL_PERMITS",
    "KANADE_MODEL_CONTEXT",
];

const USAGE: &str = "usage: v02_quality (--dry-run | --chat-alias A --chat-reasoning R --extract-alias A --extract-reasoning R) [--out DIR] [--cases C01,E07,…] [--cap N≤500] [--persona-bundle FILE]\n       v02_quality --rescore RUN_DIR [--persona-bundle FILE]";

#[derive(Default)]
struct Args {
    dry: bool,
    rescore: Option<PathBuf>,
    out: Option<PathBuf>,
    cases: Option<Vec<String>>,
    cap: Option<usize>,
    chat_alias: Option<String>,
    chat_reasoning: Option<String>,
    extract_alias: Option<String>,
    extract_reasoning: Option<String>,
    persona_bundle: Option<PathBuf>,
}

fn parse(mut raw: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args::default();
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--dry-run" => args.dry = true,
            "--rescore" => args.rescore = Some(value()?.into()),
            "--out" => args.out = Some(value()?.into()),
            "--cases" => {
                args.cases = Some(
                    value()?
                        .split(',')
                        .map(|id| id.trim().to_uppercase())
                        .collect(),
                )
            }
            "--cap" => {
                let cap: usize = value()?.parse().map_err(|_| "--cap takes a number")?;
                if cap == 0 || cap > HARD_CAP {
                    return Err(format!("--cap must be 1..={HARD_CAP}"));
                }
                args.cap = Some(cap);
            }
            "--chat-alias" => args.chat_alias = Some(value()?),
            "--chat-reasoning" => args.chat_reasoning = Some(value()?),
            "--extract-alias" => args.extract_alias = Some(value()?),
            "--extract-reasoning" => args.extract_reasoning = Some(value()?),
            "--persona-bundle" => args.persona_bundle = Some(value()?.into()),
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    if let Some(ids) = &args.cases
        && let Some(unknown) = ids
            .iter()
            .find(|id| !cases::CASES.iter().any(|case| case.id == id.as_str()))
    {
        return Err(format!("unknown case {unknown}"));
    }
    Ok(args)
}

fn stamp() -> String {
    // chrono's clock feature is not compiled in.
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let now = DateTime::<Utc>::from_timestamp(seconds, 0).unwrap_or_default();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    output::write(path, (text + "\n").as_bytes())
}

fn read_attempts(dir: &Path) -> Result<Vec<Value>, String> {
    let mut found: Vec<(String, u64, Value)> = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if !name.ends_with(".json") || name == "run.json" {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
        let case = value["case"].as_str().unwrap_or_default().to_owned();
        let attempt = value["attempt"].as_u64().unwrap_or_default();
        found.push((case, attempt, value));
    }
    found.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    Ok(found.into_iter().map(|(_, _, value)| value).collect())
}

/// The run's bundle as recorded: id, file name and digest, never its text.
fn persona_meta(persona: &PersonaBundle) -> Value {
    json!({
        "id": persona.id.as_str(),
        "bundle_file": persona.file_name,
        "sha256": persona.sha256,
        "selection": "stored `persona` row + catalog default (configured), default reply profile",
    })
}

fn rescore(dir: &Path, persona: &PersonaBundle) -> Result<ExitCode, String> {
    let meta: Value = serde_json::from_str(
        &fs::read_to_string(dir.join("run.json")).map_err(|e| format!("run.json: {e}"))?,
    )
    .map_err(|e| format!("run.json: {e}"))?;
    // The disclosure corpus must be the bundle the run used.
    if meta["persona"]["sha256"].as_str() != Some(persona.sha256.as_str()) {
        return Err(format!(
            "{} is not the bundle this run used (sha256 differs); pass --persona-bundle",
            persona.file_name
        ));
    }
    let attempts = read_attempts(dir)?;
    output::write(
        &dir.join("summary.md"),
        report::summary(&meta, &attempts).as_bytes(),
    )?;
    eprintln!(
        "rescored {} attempts: {}",
        attempts.len(),
        dir.join("summary.md").display()
    );
    Ok(ExitCode::SUCCESS)
}

/// Serve's own startup selection over the attempt layout (no model call):
/// the stored row names the bundle, so it must be selected as configured.
fn preflight(config: &ServeConfig, persona: &PersonaBundle) -> Result<(), String> {
    let load = load_personas(&config.files.persona_dir, Some(&persona.id))
        .map_err(|e| format!("persona preflight: {e}"))?;
    let provenance = load.snapshot.provenance();
    let effective = provenance
        .effective
        .as_ref()
        .map(|id| id.as_str().to_owned());
    if effective.as_deref() != Some(persona.id.as_str())
        || provenance.source != Some(SelectionSource::Configured)
    {
        return Err(format!(
            "persona preflight: serve would answer as {effective:?} via {:?}, not `{}` as configured",
            provenance.source,
            persona.id.as_str()
        ));
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("v02_quality: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode, String> {
    let args = parse(std::env::args().skip(1))?;
    let persona = Arc::new(persona::load(
        &args.persona_bundle.clone().unwrap_or_else(persona::tracked),
    )?);
    Detector::install(&persona.bundle);
    if let Some(dir) = &args.rescore {
        return rescore(dir, &persona);
    }
    tls::install_ring_provider().map_err(|e| e.to_string())?;

    let mut mapping: BTreeMap<String, String> = [
        ("KANADE_TIMEZONE", "Asia/Singapore".to_owned()),
        ("KANADE_ADMIN_BIND", "127.0.0.1:0".to_owned()),
        ("KANADE_GUILD_ID", world::GUILD.to_string()),
        ("KANADE_BOSSING_ROLE_ID", world::BOSSING.to_string()),
        ("KANADE_CHAT_PILOT_ROLE_ID", world::BOSSING.to_string()),
        ("KANADE_DISCORD_GATEWAY", "1".to_owned()),
        ("KANADE_EXPECT_V4_STOPPED", "1".to_owned()),
        ("KANADE_CHAT_ENABLED", "1".to_owned()),
        ("KANADE_CHAT_CATEGORY_IDS", world::CATEGORY.to_string()),
        ("KANADE_EXTRACTION_ENABLED", "1".to_owned()),
        (
            "KANADE_WATCH_CHANNEL_IDS",
            format!("{},{}", world::C1, world::C2),
        ),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    let mode = if args.dry {
        let url = dry::start()
            .await
            .map_err(|e| format!("dry gateway: {e}"))?;
        mapping.insert("KANADE_MODEL_BASE_URL".into(), url);
        for (key, value) in [
            ("KANADE_CHAT_MODEL", dry::CHAT_ALIAS),
            ("KANADE_CHAT_REASONING", "low"),
            ("KANADE_EXTRACT_MODEL", dry::EXTRACT_ALIAS),
            ("KANADE_EXTRACT_REASONING", "low"),
        ] {
            mapping.insert(key.into(), value.into());
        }
        "dry-run (loopback stand-in gateway)"
    } else {
        for key in MODEL_ENV {
            if let Ok(value) = std::env::var(key) {
                mapping.insert(key.into(), value);
            }
        }
        if !mapping.contains_key("KANADE_MODEL_BASE_URL") {
            return Err("KANADE_MODEL_BASE_URL is not set (or use --dry-run)".into());
        }
        for (key, value) in [
            ("KANADE_CHAT_MODEL", &args.chat_alias),
            ("KANADE_CHAT_REASONING", &args.chat_reasoning),
            ("KANADE_EXTRACT_MODEL", &args.extract_alias),
            ("KANADE_EXTRACT_REASONING", &args.extract_reasoning),
        ] {
            let value = value.as_ref().ok_or(format!(
                "the live run needs every alias and reasoning flag\n{USAGE}"
            ))?;
            mapping.insert(key.into(), value.clone());
        }
        "live"
    };

    let run_name = stamp();
    let out = args
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("docs/notes/evidence/v02").join(&run_name));
    output::prepare(&out)?;

    let cap = args.cap.unwrap_or(HARD_CAP);
    let requests = Requests::install(cap);
    let scratch = TempDir::new("models").map_err(|e| e.to_string())?;
    let config = attempt::config_in(&scratch.0, &mapping, &persona)?;
    preflight(&config, &persona)?;
    let models = harness::shared_models(&config)
        .map_err(|e| e.to_string())?
        .ok_or("no model stack (KANADE_MODEL_BASE_URL unset)")?;
    drop(scratch);
    for role in [Role::Chat, Role::Extraction] {
        if !models.has_role(role) {
            return Err(format!("the {} role has no alias", role.as_str()));
        }
    }
    let startup = models.check_startup().await;
    let refresh = models.spawn_catalog_refresh();
    let routes: Vec<Value> = [Role::Chat, Role::Extraction]
        .into_iter()
        .filter_map(|role| models.governor.route(role))
        .map(|route| {
            json!({
                "role": route.role.as_str(),
                "alias": route.alias,
                "effort": route.effort.map(|e| e.as_str()),
                "external": route.external,
                "group": route.group,
            })
        })
        .collect();
    let mut meta = json!({
        "run": run_name,
        "mode": mode,
        "draft": "docs/notes/evidence/v02-quality-set-draft-2026-10-07.md",
        "routes": routes,
        "listing": format!("{:?}", startup.listing),
        "warnings": startup.warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "groups": config.models.groups.iter().map(|g| g.name.clone()).collect::<Vec<_>>(),
        "ca_file": config.models.ca_file.is_some(),
        "attendance": "V4_COMPAT",
        // No stored row: the code default (Classic) styles reminder cards.
        "message_style": settings::seed(&config.seeds).notifications.message_style.as_str(),
        "persona": persona_meta(&persona),
        "aborted": false,
    });
    eprintln!(
        "v02: {mode}; routes {}; listing {:?}",
        meta["routes"], startup.listing
    );

    let selected: Vec<&cases::Case> = cases::CASES
        .iter()
        .filter(|case| {
            args.cases
                .as_ref()
                .is_none_or(|ids| ids.iter().any(|id| id == case.id))
        })
        .collect();
    let planned: u32 = selected.iter().map(|case| cases::attempts(case.id)).sum();
    meta["planned_attempts"] = json!(planned);
    eprintln!(
        "v02: {} cases, {planned} attempts, request cap {cap}",
        selected.len()
    );
    write_json(&out.join("run.json"), &meta)?;

    let shared = Shared {
        models: Arc::clone(&models),
        requests: Arc::clone(&requests),
        mapping,
        persona: Arc::clone(&persona),
    };
    let mut attempts = Vec::new();
    'cases: for case in selected {
        for n in 1..=cases::attempts(case.id) {
            if requests.exhausted() {
                meta["aborted"] = json!(true);
                meta["abort_reason"] =
                    json!(format!("request cap {cap} reached before {}-{n}", case.id));
                break 'cases;
            }
            let record = attempt::run(case, n, &shared).await;
            write_json(&out.join(format!("{}-{n}.json", case.id)), &record)?;
            let grade = score::score(&record).grade();
            eprintln!(
                "v02: {}-{n} {} · requests {} (run {}){}",
                case.id,
                grade.as_str(),
                record["requests"]["sent"],
                requests.sent(),
                record["error"]
                    .as_str()
                    .map(|e| format!(" · error: {e}"))
                    .unwrap_or_default()
            );
            let setup = record["setup_error"] == true;
            let reason = record["error"].as_str().unwrap_or_default().to_owned();
            attempts.push(record);
            if setup {
                meta["aborted"] = json!(true);
                meta["abort_reason"] = json!(format!("{}-{n}: {reason}", case.id));
                break 'cases;
            }
            if requests.refused() > 0 {
                meta["aborted"] = json!(true);
                meta["abort_reason"] =
                    json!(format!("request cap {cap} reached during {}-{n}", case.id));
                break 'cases;
            }
        }
    }
    refresh.abort();
    meta["completed_attempts"] = json!(attempts.len());
    meta["requests"] = json!({
        "sent": requests.sent(),
        "completions": requests.completions(),
        "cap": cap,
        "refused": requests.refused(),
        "by_path": requests.by_path(),
        "off_loopback": requests.off_loopback(),
    });
    write_json(&out.join("run.json"), &meta)?;
    output::write(
        &out.join("summary.md"),
        report::summary(&meta, &attempts).as_bytes(),
    )?;
    eprintln!(
        "v02: {} attempts, {} requests; summary {}",
        attempts.len(),
        requests.sent(),
        out.join("summary.md").display()
    );
    if args.dry && !requests.off_loopback().is_empty() {
        eprintln!("v02: a dry run sent requests off loopback");
        return Ok(ExitCode::from(3));
    }
    if meta["aborted"] == true {
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}
