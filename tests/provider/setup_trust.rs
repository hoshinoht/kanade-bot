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
        .map(|(role, alias)| StartupWarning::ExternalRefused {
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
async fn startup_reports_stranded_efforts_capacity_and_refused_external_routes() {
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
            },
            StartupWarning::ExternalRefused {
                role: Role::Extraction,
                alias: "codex-like".into(),
            },
            StartupWarning::ExternalRefused {
                role: Role::Rewrite,
                alias: "glm-cloud".into(),
            },
        ]
    );
    assert_eq!(
        report.warnings[1].to_string(),
        "extraction reasoning max is not published by codex-like; sending off"
    );
    // Stranded extraction sends off; chat inherits the configured max, which
    // sumi publishes; glm-cloud has no reasoning control, so high is legal.
    assert_eq!(stack.effort(Role::Extraction), Some(Effort::Off));
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
            effort: Effort::Off,
            stranded: Some(Effort::Xhigh)
        }
    );
    assert_eq!(stack.effort(Role::Rewrite), Some(Effort::Medium));
    assert_eq!(stack.effort(Role::Extraction), Some(Effort::Xhigh));
    let unrouted = ready(setup(Some(stub.url())));
    assert_eq!(unrouted.effort(Role::Rewrite), None);
}

#[tokio::test]
async fn the_override_is_reported_loudly_for_each_external_route() {
    let stub = Stub::start(gateway(kanata_models(), "{}")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = roles("codex-like");
    input.allow_external_unmasked = true;
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
    let route = stack.governor.route(Role::Chat).unwrap();
    assert!(route.external && route.unmasked_allowed);
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
