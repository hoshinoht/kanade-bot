use std::collections::BTreeMap;

use kanade::{
    cli::models::{Args, check},
    infrastructure::llm::{
        Effort,
        governor::Role,
        setup::{ModelRoles, RoleEffort, Variant, variant_of},
    },
};
use serde_json::{Value, json};

use super::setup::{ready, role, setup};
use super::stub::{Stub, gateway};

fn variants() -> Value {
    let reasoning = json!({"reasoning_control": true, "reasoning_efforts": ["low", "medium", "high"], "trust_zone": "local"});
    json!({"object": "list", "data": [
        {"id": "gpt-6-luna", "kanata": reasoning},
        {"id": "gpt-6-luna:high", "kanata": reasoning},
        {"id": "gpt-6-luna:low", "kanata": reasoning},
        {"id": "gpt-6-luna:medium", "kanata": reasoning},
        {"id": "gpt-oss:120b-cloud", "kanata": {"trust_zone": "local"}},
        {"id": "gpt-5.6-sol:low", "kanata": reasoning},
    ]})
}

#[test]
fn only_a_known_effort_suffix_on_a_listed_base_is_a_variant() {
    let listed = |alias: &str| ["gpt-6-luna", "gpt-oss", "base"].contains(&alias);
    assert_eq!(
        variant_of("gpt-6-luna:high", listed),
        Some(Variant {
            base: "gpt-6-luna".into(),
            effort: Effort::High
        })
    );
    assert_eq!(
        variant_of("base:none", listed).map(|variant| variant.effort),
        Some(Effort::Off)
    );
    assert_eq!(
        variant_of("gpt-oss:120b-cloud", listed),
        None,
        "not an effort"
    );
    assert_eq!(
        variant_of("gpt-5.6-sol:low", listed),
        None,
        "base not listed"
    );
    assert_eq!(
        variant_of("base:off", listed),
        None,
        "Kanata's name is none"
    );
    assert_eq!(variant_of(":high", |_| true), None);
    assert_eq!(variant_of("gpt-6-luna", listed), None);
}

#[tokio::test]
async fn a_variant_role_uses_its_fixed_level_and_inheritors_follow_it() {
    let stub = Stub::start(gateway(variants(), "ok")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles {
        extraction: role("gpt-6-luna:high", RoleEffort::Level(Effort::Low)),
        chat: role("gpt-6-luna", RoleEffort::Inherit),
        rewrite: role("gpt-6-luna:medium", RoleEffort::Level(Effort::Off)),
    };
    let stack = ready(input);
    stack.check_startup().await;
    let efforts = stack.efforts();
    assert_eq!(efforts[&Role::Extraction].effort, Effort::High);
    assert_eq!(efforts[&Role::Chat].effort, Effort::High);
    assert_eq!(efforts[&Role::Rewrite].effort, Effort::Medium);
    assert!(efforts.values().all(|status| status.stranded.is_none()));
    let catalog = stack.catalog();
    assert_eq!(
        catalog.variant("gpt-6-luna:low").unwrap().base,
        "gpt-6-luna"
    );
    assert_eq!(catalog.variant("gpt-5.6-sol:low"), None);
}

#[tokio::test]
async fn models_check_groups_variants_under_their_base() {
    let _ = kanade::runtime::tls::install_ring_provider();
    let stub = Stub::start(gateway(variants(), "ok")).await;
    let env = BTreeMap::from([
        ("KANADE_MODEL_BASE_URL".to_owned(), stub.url()),
        ("KANADE_CHAT_MODEL".to_owned(), "gpt-6-luna:high".to_owned()),
    ]);
    let mut out = Vec::new();
    check(Args { probe: false }, &env, &mut out).await.unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains(
            "  gpt-6-luna zone=local homelab=stays efforts=low,medium,high (off not allowed) \
             variants: high, low, medium\n"
        ),
        "{out}"
    );
    assert!(!out.contains("  gpt-6-luna:high zone="), "{out}");
    assert!(out.contains("  gpt-oss:120b-cloud zone=local"), "{out}");
    assert!(
        out.contains("  gpt-5.6-sol:low zone=local"),
        "base not listed"
    );
    assert!(
        out.contains("  chat gpt-6-luna:high effort=high (fixed) route="),
        "{out}"
    );
}
