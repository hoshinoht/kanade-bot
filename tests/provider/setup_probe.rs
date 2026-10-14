use std::collections::BTreeMap;

use kanade::{
    cli::models::{Args, check, parse},
    infrastructure::llm::{
        Effort,
        governor::Role,
        setup::{ModelRoles, ProbeOutcome, RoleEffort},
    },
    runtime::error::Error,
};
use serde_json::json;
use std::time::Duration;

use super::setup::{ready, role, roles, setup, temp_file};
use super::stub::{Reply, Stub, gateway, kanata_models};

const KEY: &str = "sk-models-check-sentinel";

#[tokio::test]
async fn a_probe_sends_one_tiny_member_free_completion_per_role() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles {
        rewrite: role("sumi-structured", RoleEffort::Level(Effort::Low)),
        ..roles("sumi-structured")
    };
    let stack = ready(input);
    stack.check_startup().await;
    let mut results = Vec::new();
    for role in ModelRoles::ALL {
        let result = stack.probe(role, Duration::from_secs(5)).await.unwrap();
        assert!(
            matches!(&result.outcome, ProbeOutcome::Ok { finish_reason, .. } if finish_reason == "stop"),
            "{result:?}"
        );
        assert_eq!(result.request_ids.len(), 1, "{result:?}");
        results.push(result);
    }
    let sent = stub.chat_requests();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].body["max_tokens"], 128);
    let headers: Vec<&str> = sent
        .iter()
        .map(|request| request.header("x-request-id").expect("tagged"))
        .collect();
    assert_eq!(
        results
            .iter()
            .flat_map(|result| &result.request_ids)
            .map(String::as_str)
            .collect::<Vec<_>>(),
        headers,
        "each probe reports the id it sent"
    );
    assert_eq!(sent[0].body["reasoning_effort"], "none");
    assert_eq!(sent[2].body["reasoning_effort"], "low");
    assert!(sent[0].body.get("tools").is_none());
    assert_eq!(sent[0].body["messages"][1]["content"], "ping");
}

#[tokio::test]
async fn an_external_route_probe_sends_its_fixed_ping_without_opt_in() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = roles("codex-like");
    let stack = ready(input);
    stack.check_startup().await;
    let result = stack
        .probe(Role::Chat, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(result.outcome.is_ok(), "{result:?}");
    assert_eq!(stub.chat_requests().len(), 1);
    assert_eq!(
        stub.chat_requests()[0].body["messages"][1]["content"],
        "ping"
    );
}

fn env(url: &str, pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    let mut env = BTreeMap::from([("KANADE_MODEL_BASE_URL".to_owned(), url.to_owned())]);
    env.extend(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    env
}

async fn run(args: Args, env: &BTreeMap<String, String>) -> (String, Result<(), Error>) {
    let _ = kanade::runtime::tls::install_ring_provider();
    let mut out = Vec::new();
    let result = check(args, env, &mut out).await;
    (String::from_utf8(out).unwrap(), result)
}

#[test]
fn the_models_command_parses_check_and_probe() {
    let args = |list: &[&str]| parse(&list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
    assert_eq!(args(&["check"]).unwrap(), Args { probe: false });
    assert_eq!(args(&["check", "--probe"]).unwrap(), Args { probe: true });
    assert!(args(&[]).is_err());
    assert!(args(&["check", "--other"]).is_err());
    assert_eq!(
        kanade::cli::parse(["models".into(), "check".into()]).unwrap(),
        kanade::cli::Command::Models(Args { probe: false })
    );
}

#[tokio::test]
async fn models_check_lists_the_catalog_and_routes_without_printing_the_key() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    let key = temp_file(format!("{KEY}\n").as_bytes());
    let env = env(
        &stub.url(),
        &[
            ("KANADE_MODEL_KEY_FILE", key.to_str().unwrap()),
            ("KANADE_EXTRACT_MODEL", "sumi-structured"),
            ("KANADE_CHAT_MODEL", "codex-like"),
            ("KANADE_REWRITE_MODEL", "gone"),
        ],
    );
    let (out, result) = run(Args { probe: false }, &env).await;
    std::fs::remove_file(key).unwrap();
    result.unwrap();
    assert!(!out.contains(KEY));
    assert!(out.contains("(key: set, roots: webpki)"), "{out}");
    assert!(out.contains("catalog: 6 models"), "{out}");
    assert!(out.contains(
        "  sumi-structured zone=private_network homelab=stays \
         efforts=off,minimal,low,medium,high,xhigh,max context=32768 max_output=4096 in_flight=2"
    ));
    assert!(out.contains("  glm-cloud zone=local homelab=leaves efforts=unsupported"));
    assert!(out.contains("  extraction sumi-structured effort=off route=homelab"));
    assert!(out.contains(
        "  codex-like zone=external homelab=leaves efforts=low,medium,high (off not allowed) in_flight=8"
    ));
    assert!(out.contains("  chat codex-like effort=low (configured off) route=external_unmasked"));
    assert!(out.contains(
        "warning: chat reasoning off is not allowed: codex-like requires reasoning; sending low"
    ));
    assert!(out.contains("  rewrite gone (not listed) effort=off route=external_unmasked"));
    assert!(out.contains("warning: UNMASKED: chat model codex-like leaves the homelab"));
    assert!(!out.contains("probe:"));
    assert!(stub.chat_requests().is_empty());
}

#[tokio::test]
async fn models_check_probe_sends_ping_to_external_roles_without_opt_in() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    let base = [
        ("KANADE_EXTRACT_MODEL", "sumi-structured"),
        ("KANADE_CHAT_MODEL", "codex-like"),
    ];
    let (out, result) = run(Args { probe: true }, &env(&stub.url(), &base)).await;
    result.unwrap();
    assert!(
        out.contains("  extraction sumi-structured effort=off ok "),
        "{out}"
    );
    assert!(out.contains("finish=stop"));
    assert!(out.contains("route=external_unmasked"));
    assert!(out.contains("warning: UNMASKED: chat model codex-like"));
    assert!(out.contains("  chat codex-like effort=low ok "));
    let requests = stub.chat_requests();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| { request.body["messages"][1]["content"] == "ping" })
    );
    // The listing names each probe's sent id, so the gateway log can be searched.
    for request in &requests {
        let id = request.header("x-request-id").expect("tagged");
        assert!(out.contains(&format!(" request_ids={id}")), "{out}");
    }
}

#[tokio::test]
async fn models_check_fails_when_the_listing_fails_or_a_retired_policy_key_is_present() {
    let stub = Stub::start(|_: &super::stub::Recorded| Reply::Json(500, json!({}))).await;
    let (out, result) = run(Args { probe: false }, &env(&stub.url(), &[])).await;
    assert!(out.contains("catalog: unavailable (server-error)"), "{out}");
    assert!(matches!(result, Err(Error::Unavailable(_))));

    let (_, result) = run(Args { probe: false }, &BTreeMap::new()).await;
    assert!(matches!(result, Err(Error::Configuration(_))));
    let (_, result) = run(
        Args { probe: false },
        &env(&stub.url(), &[("KANADE_PSEUDONYMIZE", "")]),
    )
    .await;
    assert!(matches!(result, Err(Error::Configuration(_))));
}

#[tokio::test]
async fn models_check_uses_the_env_reasoning_seeds() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    let env = env(
        &stub.url(),
        &[
            ("KANADE_EXTRACT_MODEL", "sumi-structured"),
            ("KANADE_EXTRACT_REASONING", "medium"),
            ("KANADE_CHAT_MODEL", "sumi-structured"),
            ("KANADE_CHAT_REASONING", "inherit"),
            ("KANADE_REWRITE_MODEL", "codex-like"),
            ("KANADE_REWRITE_REASONING", "high"),
        ],
    );
    let (out, result) = run(Args { probe: false }, &env).await;
    result.unwrap();
    assert!(
        out.contains("roles (env seeds; saved settings are not read):"),
        "{out}"
    );
    assert!(
        out.contains("  extraction sumi-structured effort=medium route=homelab"),
        "{out}"
    );
    assert!(
        out.contains("  chat sumi-structured effort=medium route=homelab"),
        "{out}"
    );
    assert!(
        out.contains("  rewrite codex-like effort=high route=external_unmasked"),
        "{out}"
    );
}

#[tokio::test]
async fn models_check_prints_each_role_context_and_warns_only_for_large_local_windows() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    let warning = "warning: Context past 16k may result in degraded performance on local models.";
    let base = [
        // Private network, publishes 32768 with a 4096 output maximum.
        ("KANADE_EXTRACT_MODEL", "sumi-structured"),
        ("KANADE_CHAT_MODEL", "codex-like"),
        ("KANADE_REWRITE_MODEL", "codex-like"),
    ];
    let (out, result) = run(Args { probe: false }, &env(&stub.url(), &base)).await;
    result.unwrap();
    assert!(
        out.contains(&format!(
            "  extraction sumi-structured effort=off route=homelab\n    context=32768 reserve=2500 source=catalog\n{warning}\n"
        )),
        "{out}"
    );
    assert_eq!(
        out.matches(warning).count(),
        1,
        "cloud routes never warn: {out}"
    );
    assert!(
        out.contains("    context=65536 reserve=96 source=cloud_default"),
        "{out}"
    );

    // A seeded extraction cap at the threshold ends the warning.
    let seed = r#"{"cloud_default":65536,"local_default":8192,"chat":{"reserve":1024},"extraction":{"reserve":2500,"cap":16384},"rewrite":{"reserve":96}}"#;
    let mut capped = base.to_vec();
    capped.push(("KANADE_MODEL_CONTEXT", seed));
    let (out, result) = run(Args { probe: false }, &env(&stub.url(), &capped)).await;
    result.unwrap();
    assert!(
        out.contains("    context=16384 reserve=2500 source=catalog"),
        "{out}"
    );
    assert!(!out.contains(warning), "{out}");
}

#[tokio::test]
async fn models_check_warns_when_a_routed_reserve_fills_its_effective_window() {
    let stub = Stub::start(gateway(kanata_models(), "ok")).await;
    // An override narrower than extraction's 2500 reserve.
    let seed = r#"{"cloud_default":65536,"local_default":8192,"chat":{"reserve":1024},"extraction":{"reserve":2500},"rewrite":{"reserve":96},"overrides":{"sumi-structured":2048}}"#;
    let pairs = [
        ("KANADE_EXTRACT_MODEL", "sumi-structured"),
        ("KANADE_CHAT_MODEL", "codex-like"),
        ("KANADE_MODEL_CONTEXT", seed),
    ];
    let (out, result) = run(Args { probe: false }, &env(&stub.url(), &pairs)).await;
    result.unwrap();
    assert!(
        out.contains(
            "    context=2048 reserve=2500 source=override\nwarning: extraction context reserve 2500 is not smaller than sumi-structured's effective window 2048; its prompts cannot fit\n"
        ),
        "{out}"
    );
    assert_eq!(out.matches("prompts cannot fit").count(), 1, "{out}");
}
