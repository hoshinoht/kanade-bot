use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use kanade::infrastructure::llm::{
    Effort, TrustZone,
    governor::{ConfigWarning, Role},
    setup::{
        EffortStatus, Listing, ModelRoles, ModelStack, RoleEffort, StartupWarning, leaves_homelab,
    },
};
use serde_json::{Value, json};

use super::setup::{ready, role, roles, setup};
use super::stub::{Reply, Stub, gateway, kanata_models};

fn listing(zone: &str) -> Value {
    json!({"object": "list", "data": [
        {"id": "home", "kanata": {"trust_zone": zone}},
        {"id": "home-cloud", "kanata": {"trust_zone": "local"}},
    ]})
}

/// `GET /models` answers from `listings[state]`; `None` is a 500.
async fn switching(listings: Vec<Option<Value>>, state: Arc<AtomicUsize>) -> Stub {
    let handler = move |request: &super::stub::Recorded| {
        if !request.path.ends_with("/models") {
            return Reply::Json(500, json!({}));
        }
        match &listings[state.load(Ordering::SeqCst)] {
            Some(body) => Reply::Json(200, body.clone()),
            None => Reply::Json(500, json!({"error": {"message": "down"}})),
        }
    };
    Stub::start(handler).await
}

fn external(stack: &ModelStack, role: Role) -> bool {
    stack.governor.route(role).unwrap().external
}

#[test]
fn only_a_published_home_zone_without_a_cloud_suffix_stays_home() {
    use kanade::infrastructure::llm::ModelCapabilities;
    let zoned = |zone| ModelCapabilities {
        trust_zone: zone,
        ..ModelCapabilities::minimal()
    };
    assert!(!leaves_homelab("a", Some(&zoned(Some(TrustZone::Local)))));
    assert!(!leaves_homelab(
        "a",
        Some(&zoned(Some(TrustZone::PrivateNetwork)))
    ));
    assert!(leaves_homelab("a", Some(&zoned(Some(TrustZone::External)))));
    assert!(leaves_homelab("a", Some(&zoned(None))));
    assert!(leaves_homelab("a", None));
    assert!(leaves_homelab(
        "a-cloud",
        Some(&zoned(Some(TrustZone::Local)))
    ));
}

#[tokio::test]
async fn routes_follow_each_listing_and_stay_external_until_one_succeeds() {
    let state = Arc::new(AtomicUsize::new(0));
    let stub = switching(
        vec![
            None,
            Some(listing("local")),
            Some(listing("external")),
            None,
        ],
        state.clone(),
    )
    .await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles {
        extraction: role("home", RoleEffort::Level(Effort::Off)),
        chat: role("home-cloud", RoleEffort::Inherit),
        rewrite: role("unlisted", RoleEffort::Inherit),
    };
    let stack = ready(input);
    assert!(
        external(&stack, Role::Extraction),
        "fail closed before a listing"
    );

    let report = stack.check_startup().await;
    assert!(matches!(report.listing, Listing::Degraded { .. }));
    assert!(external(&stack, Role::Extraction));
    assert_eq!(
        report.warnings,
        [
            (Role::Extraction, "home"),
            (Role::Chat, "home-cloud"),
            (Role::Rewrite, "unlisted")
        ]
        .map(|(role, alias)| StartupWarning::ExternalUnmasked {
            role,
            alias: alias.into()
        })
    );
    assert!(!stack.catalog().listed);

    state.store(1, Ordering::SeqCst);
    stack.provider.list_models().await.unwrap();
    assert!(!external(&stack, Role::Extraction));
    assert!(external(&stack, Role::Chat), "-cloud always leaves");
    assert!(external(&stack, Role::Rewrite), "unlisted fails closed");

    state.store(2, Ordering::SeqCst);
    stack.provider.list_models().await.unwrap();
    assert!(external(&stack, Role::Extraction), "re-derived on refresh");

    state.store(1, Ordering::SeqCst);
    stack.provider.list_models().await.unwrap();
    state.store(3, Ordering::SeqCst);
    assert!(stack.provider.list_models().await.is_err());
    assert!(
        !external(&stack, Role::Extraction),
        "a failed refresh keeps the last derivation"
    );
}

#[tokio::test]
async fn startup_reports_stranded_efforts_capacity_and_unmasked_external_routes() {
    let stub = Stub::start(gateway(kanata_models(), "{}")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles {
        extraction: role("codex-like", RoleEffort::Level(Effort::Max)),
        chat: role("sumi-structured", RoleEffort::Inherit),
        rewrite: role("glm-cloud", RoleEffort::Level(Effort::High)),
    };
    input.permits = 4;
    let stack = ready(input);
    let report = stack.check_startup().await;
    assert!(matches!(report.listing, Listing::Listed { .. }));
    assert_eq!(
        report.warnings,
        vec![
            StartupWarning::Governor(ConfigWarning::PermitsAboveGateway {
                group: "gateway".into(),
                alias: "sumi-structured".into(),
                permits: 4,
                gateway: 2,
            }),
            StartupWarning::UnpublishedEffort {
                role: Role::Extraction,
                alias: "codex-like".into(),
                effort: Effort::Max,
                sent: Effort::Low,
            },
            StartupWarning::ExternalUnmasked {
                role: Role::Extraction,
                alias: "codex-like".into(),
            },
            StartupWarning::ExternalUnmasked {
                role: Role::Rewrite,
                alias: "glm-cloud".into(),
            },
        ]
    );
    assert_eq!(
        report.warnings[1].to_string(),
        "extraction reasoning max is not published by codex-like; sending low"
    );
    // Stranded extraction falls to codex-like's lowest level (it requires
    // reasoning); chat inherits the configured max, which sumi publishes;
    // glm-cloud has no reasoning control, so high is legal.
    assert_eq!(stack.effort(Role::Extraction), Some(Effort::Low));
    assert_eq!(stack.effort(Role::Chat), Some(Effort::Max));
    assert_eq!(stack.effort(Role::Rewrite), Some(Effort::High));
}

#[tokio::test]
async fn an_inherited_level_is_checked_against_the_inheriting_alias() {
    let stub = Stub::start(gateway(kanata_models(), "{}")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles {
        extraction: role("sumi-structured", RoleEffort::Level(Effort::Xhigh)),
        chat: role("codex-like", RoleEffort::Inherit),
        rewrite: role("codex-like", RoleEffort::Level(Effort::Medium)),
    };
    let stack = ready(input);
    assert_eq!(
        stack.efforts()[&Role::Chat],
        EffortStatus {
            effort: Effort::Xhigh,
            stranded: None
        },
        "unchecked before a listing"
    );
    stack.check_startup().await;
    assert_eq!(
        stack.efforts()[&Role::Chat],
        EffortStatus {
            effort: Effort::Low,
            stranded: Some(Effort::Xhigh)
        }
    );
    assert_eq!(stack.effort(Role::Rewrite), Some(Effort::Medium));
    assert_eq!(stack.effort(Role::Extraction), Some(Effort::Xhigh));
    let unrouted = ready(setup(Some(stub.url())));
    assert_eq!(unrouted.effort(Role::Rewrite), None);
}

#[tokio::test]
async fn every_external_route_reports_raw_data_and_a_zdr_transmission_warning() {
    let stub = Stub::start(gateway(kanata_models(), "{}")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = roles("codex-like");
    let stack = ready(input);
    let report = stack.check_startup().await;
    let unmasked: Vec<_> = report
        .warnings
        .iter()
        .filter(|warning| matches!(warning, StartupWarning::ExternalUnmasked { .. }))
        .collect();
    assert_eq!(unmasked.len(), 2, "{:?}", report.warnings);
    assert!(
        unmasked[0]
            .to_string()
            .starts_with("UNMASKED: extraction model codex-like")
    );
    let warning = unmasked[0].to_string();
    for detail in [
        "member names",
        "IDs",
        "messages",
        "complete URLs",
        "Kanata ZDR",
        "does not prevent transmission",
    ] {
        assert!(warning.contains(detail), "missing {detail}: {warning}");
    }
    let route = stack.governor.route(Role::Chat).unwrap();
    assert!(route.external);
    assert_eq!(stack.route_kind(Role::Chat), Some("external_unmasked"));
}

#[tokio::test]
async fn the_catalog_snapshot_exposes_published_metadata() {
    let stub = Stub::start(gateway(kanata_models(), "{}")).await;
    let stack = ready(setup(Some(stub.url())));
    assert_eq!(stack.catalog().models.len(), 0);
    stack.check_startup().await;
    let catalog = stack.catalog();
    assert!(catalog.listed);
    let json = serde_json::to_value(&catalog).unwrap();
    let find = |alias: &str| {
        json["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["alias"] == alias)
            .unwrap()
            .clone()
    };
    let sumi = find("sumi-structured");
    assert_eq!(sumi["trust_zone"], "private_network");
    assert_eq!(sumi["leaves_homelab"], false);
    assert_eq!(sumi["published"], true);
    assert_eq!(
        sumi["reasoning_efforts"],
        json!(["off", "minimal", "low", "medium", "high", "xhigh", "max"])
    );
    assert_eq!(sumi["context_tokens"], 32768);
    assert_eq!(sumi["admission"]["adapter_max_in_flight"], 2);
    assert_eq!(find("glm-cloud")["leaves_homelab"], true);
    let plain = find("plain-alias");
    assert_eq!(
        (plain["published"].clone(), plain["leaves_homelab"].clone()),
        (json!(false), json!(true))
    );
    assert_eq!(find("odd-flags")["trust_zone"], Value::Null);
}

#[tokio::test]
async fn off_needs_a_list_with_none_else_the_lowest_published_level_is_used() {
    let stub = Stub::start(gateway(
        json!({"object": "list", "data": [
            {"id": "needs", "kanata": {"reasoning_control": true, "reasoning_efforts": ["high", "medium"]}},
            {"id": "any", "kanata": {"reasoning_control": true}},
            {"id": "none-ok", "kanata": {"reasoning_control": true, "reasoning_efforts": ["none", "high"]}},
            {"id": "no-reasoning", "kanata": {"reasoning_control": false}},
        ]}),
        "{}",
    ))
    .await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles {
        extraction: role("needs", RoleEffort::Level(Effort::Off)),
        chat: role("any", RoleEffort::Inherit),
        rewrite: role("none-ok", RoleEffort::Level(Effort::Low)),
    };
    let stack = ready(input);
    let report = stack.check_startup().await;
    let status = |effort, stranded| EffortStatus { effort, stranded };
    let efforts = stack.efforts();
    assert_eq!(
        efforts[&Role::Extraction],
        status(Effort::Medium, Some(Effort::Off))
    );
    assert_eq!(
        efforts[&Role::Chat],
        status(Effort::Off, None),
        "null list keeps off"
    );
    assert_eq!(
        efforts[&Role::Rewrite],
        status(Effort::Off, Some(Effort::Low)),
        "a list with none falls back to off"
    );
    assert_eq!(
        report.warnings[0].to_string(),
        "extraction reasoning off is not allowed: needs requires reasoning; sending medium"
    );
    let catalog = stack.catalog();
    let off = |alias: &str| {
        catalog
            .models
            .iter()
            .find(|model| model.alias == alias)
            .unwrap()
            .off_allowed()
    };
    assert!(!off("needs"));
    assert!(off("any") && off("none-ok") && off("no-reasoning"));

    let mut unsupported = setup(Some(stub.url()));
    unsupported.roles = roles("no-reasoning");
    let stack = ready(unsupported);
    stack.check_startup().await;
    assert_eq!(stack.effort(Role::Extraction), Some(Effort::Off));
}
