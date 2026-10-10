//! Saved role models switch the running stack: the next session of a role
//! opens with the new alias and effort, open sessions keep theirs, and a
//! new alias is grouped and fails closed until a listing shows its zone.

use std::{sync::Arc, time::Duration};

use kanade::api::admin::config::ModelCatalog;
use kanade::infrastructure::llm::{
    ChatRequest, Effort, Message,
    governor::{QuestionLimits, Refused, Role, SessionFailure, XorShift},
    setup::{
        CapacityGroup, ModelRoles, ModelStack, Models, RoleEffort, RoleModel, RunningRole,
        build_with_groups,
    },
};
use serde_json::{Value, json};

use super::setup::{ready, role, setup};
use super::stub::{Stub, gateway};

fn listing() -> Value {
    let zoned = |zone: &str, efforts: Value| json!({"trust_zone": zone, "reasoning_control": true, "reasoning_efforts": efforts});
    json!({"object": "list", "data": [
        {"id": "home-a", "kanata": zoned("local", json!(["none", "low", "medium", "high"]))},
        {"id": "home-b", "kanata": zoned("local", json!(["low", "medium"]))},
        {"id": "cloud-c", "kanata": zoned("external", json!(["low"]))},
    ]})
}

fn roles(chat: RoleModel) -> ModelRoles {
    ModelRoles {
        extraction: role("home-a", RoleEffort::Level(Effort::Off)),
        chat,
        rewrite: role("home-a", RoleEffort::Inherit),
    }
}

async fn stack(stub: &Stub, chat: RoleModel) -> ModelStack {
    let mut input = setup(Some(stub.url()));
    input.roles = roles(chat);
    let stack = ready(input);
    stack.provider.list_models().await.unwrap();
    stack
}

fn request(alias: &str, effort: Option<Effort>) -> ChatRequest {
    ChatRequest {
        model: alias.into(),
        messages: vec![Message::User {
            content: "SyntheticMember 999000111222333444 https://synthetic.invalid/member".into(),
        }],
        tools: Vec::new(),
        output_schema: None,
        max_output_tokens: 64,
        reasoning: effort,
        sampling: None,
    }
}

/// One question the way chat asks it: the route's alias and effort, read once.
async fn ask(stack: &ModelStack) -> Result<(), SessionFailure> {
    let route = stack.governor.route(Role::Chat).unwrap();
    let limits = QuestionLimits::new(Duration::from_secs(10));
    let mut session = stack
        .client
        .open_question_on(&route, "member", false, limits)
        .await
        .map_err(|error| error.failure)?;
    session
        .complete(&request(&route.alias, route.effort))
        .await
        .map(drop)
        .map_err(|error| error.failure)
}

fn last_sent(stub: &Stub) -> (String, Option<String>) {
    let body = stub.chat_requests().pop().unwrap().body;
    (
        body["model"].as_str().unwrap().to_owned(),
        body["reasoning_effort"].as_str().map(str::to_owned),
    )
}

fn running(alias: &str, effort: Effort) -> Option<RunningRole> {
    Some(RunningRole {
        alias: alias.into(),
        effort: Some(effort),
    })
}

#[tokio::test]
async fn a_saved_effort_is_sent_by_the_next_question_without_a_restart() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let stack = stack(&stub, role("home-a", RoleEffort::Level(Effort::Low))).await;
    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub), ("home-a".into(), Some("low".into())));

    let swaps = stack
        .apply_roles(roles(role("home-a", RoleEffort::Level(Effort::High))))
        .unwrap();
    assert_eq!(swaps.len(), 1);
    assert_eq!(swaps[0].role, Role::Chat);
    assert_eq!(swaps[0].before, running("home-a", Effort::Low));
    assert_eq!(swaps[0].after, running("home-a", Effort::High));
    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub), ("home-a".into(), Some("high".into())));

    // Extraction and rewrite routes carry their live level the same way.
    let mut next = roles(role("home-a", RoleEffort::Level(Effort::High)));
    next.extraction.effort = RoleEffort::Level(Effort::Medium);
    stack.apply_roles(next).unwrap();
    let effort = |role| stack.governor.route(role).unwrap().effort;
    assert_eq!(effort(Role::Extraction), Some(Effort::Medium));
    assert_eq!(effort(Role::Rewrite), Some(Effort::Medium), "inherits");
    assert_eq!(stack.roles().rewrite.effort, RoleEffort::Inherit);
}

#[tokio::test]
async fn an_alias_switch_reaches_the_next_session_and_open_ones_keep_theirs() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let stack = stack(&stub, role("home-a", RoleEffort::Level(Effort::Low))).await;
    let limits = QuestionLimits::new(Duration::from_secs(10));
    let mut open = stack
        .client
        .open_question("first", false, limits)
        .await
        .unwrap();
    assert_eq!(open.alias(), Some("home-a"));

    stack
        .apply_roles(roles(role("home-b", RoleEffort::Level(Effort::Medium))))
        .unwrap();
    assert_eq!(
        stack.running()[&Role::Chat],
        running("home-b", Effort::Medium).unwrap()
    );
    open.complete(&request("home-a", Some(Effort::Low)))
        .await
        .unwrap();
    assert_eq!(last_sent(&stub), ("home-a".into(), Some("low".into())));
    drop(open);

    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub), ("home-b".into(), Some("medium".into())));
    // The default group follows the routed aliases.
    let groups = stack.governor.snapshot(chrono::DateTime::UNIX_EPOCH);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "gateway");
    assert_eq!(groups[0].models, ["home-a", "home-b"]);
}

#[tokio::test]
async fn a_level_the_new_alias_does_not_publish_falls_to_its_floor() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let stack = stack(&stub, role("home-a", RoleEffort::Level(Effort::High))).await;
    stack
        .apply_roles(roles(role("home-b", RoleEffort::Level(Effort::High))))
        .unwrap();
    let status = stack.efforts()[&Role::Chat];
    assert_eq!(status.effort, Effort::Low);
    assert_eq!(status.stranded, Some(Effort::High));
    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub), ("home-b".into(), Some("low".into())));
}

#[tokio::test]
async fn a_cloud_switch_sends_raw_data_without_an_opt_in() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let stack = stack(&stub, role("home-a", RoleEffort::Level(Effort::Low))).await;
    assert_eq!(stack.route_kind(Role::Chat), Some("homelab"));
    stack
        .apply_roles(roles(role("cloud-c", RoleEffort::Level(Effort::Low))))
        .unwrap();
    assert_eq!(stack.route_kind(Role::Chat), Some("external_unmasked"));
    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub), ("cloud-c".into(), Some("low".into())));
    let request = stub.chat_requests().pop().unwrap().body;
    let serialized = request.to_string();
    for raw in [
        "SyntheticMember",
        "999000111222333444",
        "https://synthetic.invalid/member",
    ] {
        assert!(serialized.contains(raw), "{raw} missing from {serialized}");
    }
}

#[tokio::test]
async fn a_new_alias_stays_external_until_a_listing_confirms_it() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = roles(role("home-a", RoleEffort::Level(Effort::Low)));
    let stack = ready(input);
    stack
        .apply_roles(roles(role("home-b", RoleEffort::Level(Effort::Low))))
        .unwrap();
    assert_eq!(stack.route_kind(Role::Chat), Some("external_unmasked"));
    stack.provider.list_models().await.unwrap();
    assert_eq!(stack.route_kind(Role::Chat), Some("homelab"));
}

#[tokio::test]
async fn declared_groups_take_a_listed_alias_and_refuse_an_ungrouped_one() {
    let _ = kanade::runtime::tls::install_ring_provider();
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = roles(role("home-a", RoleEffort::Level(Effort::Low)));
    let groups = [
        CapacityGroup {
            name: "local".into(),
            permits: 2,
            aliases: vec!["home-a".into()],
        },
        CapacityGroup {
            name: "spare".into(),
            permits: 1,
            aliases: vec!["home-b".into()],
        },
    ];
    let Models::Ready(stack) =
        build_with_groups(input, &groups, Arc::new(XorShift::new(7))).unwrap()
    else {
        panic!("models unavailable");
    };
    stack.provider.list_models().await.unwrap();
    stack
        .apply_roles(roles(role("home-b", RoleEffort::Level(Effort::Low))))
        .unwrap();
    let route = stack.governor.route(Role::Chat).unwrap();
    assert_eq!(route.group.as_deref(), Some("spare"));
    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub).0, "home-b");

    stack
        .apply_roles(roles(role("cloud-c", RoleEffort::Level(Effort::Low))))
        .unwrap();
    assert_eq!(stack.governor.route(Role::Chat).unwrap().group, None);
    assert_eq!(
        ask(&stack).await,
        Err(SessionFailure::Refused(Refused::Ungrouped))
    );
    let names: Vec<String> = stack
        .governor
        .snapshot(chrono::DateTime::UNIX_EPOCH)
        .into_iter()
        .map(|group| group.name)
        .collect();
    assert_eq!(names, ["local", "spare"], "declared groups never grow");
}

#[tokio::test]
async fn a_role_unrouted_at_startup_gets_the_default_group_live() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let mut input = setup(Some(stub.url()));
    input.roles = ModelRoles::default();
    let stack = ready(input);
    stack.provider.list_models().await.unwrap();
    assert!(
        stack
            .governor
            .snapshot(chrono::DateTime::UNIX_EPOCH)
            .is_empty()
    );
    assert!(!stack.has_role(Role::Chat));
    assert!(!stack.routed_at_start(Role::Chat));

    let next = ModelRoles {
        chat: role("home-a", RoleEffort::Level(Effort::Medium)),
        ..ModelRoles::default()
    };
    stack.apply_roles(next).unwrap();
    ask(&stack).await.unwrap();
    assert_eq!(last_sent(&stub), ("home-a".into(), Some("medium".into())));
    let groups = stack.governor.snapshot(chrono::DateTime::UNIX_EPOCH);
    assert_eq!(groups[0].name, "gateway");
    assert_eq!(groups[0].permits.total, 2);

    // Extraction and heading rewrites are composed at startup only: a first
    // model for them waits for a restart and is not shown as running.
    let mut first = ModelRoles {
        chat: role("home-a", RoleEffort::Level(Effort::Medium)),
        ..ModelRoles::default()
    };
    first.extraction.alias = Some("home-b".into());
    first.extraction.effort = RoleEffort::Level(Effort::Low);
    stack.apply_roles(first).unwrap();
    assert!(stack.has_role(Role::Extraction));
    assert_eq!(ModelCatalog::awaiting_restart(&stack), [Role::Extraction]);
    let running = ModelCatalog::running(&stack);
    assert!(running.contains_key(&Role::Chat));
    assert!(!running.contains_key(&Role::Extraction));

    // Clearing the alias unroutes the role and empties the group.
    stack.apply_roles(ModelRoles::default()).unwrap();
    assert!(!stack.has_role(Role::Chat));
    assert!(
        stack.governor.snapshot(chrono::DateTime::UNIX_EPOCH)[0]
            .models
            .is_empty()
    );
}

#[tokio::test]
async fn an_extraction_that_inherits_is_refused_and_nothing_moves() {
    let stub = Stub::start(gateway(listing(), "ok")).await;
    let stack = stack(&stub, role("home-a", RoleEffort::Level(Effort::Low))).await;
    let mut next = roles(role("home-b", RoleEffort::Level(Effort::Low)));
    next.extraction.effort = RoleEffort::Inherit;
    assert!(stack.apply_roles(next).is_err());
    assert_eq!(stack.governor.route(Role::Chat).unwrap().alias, "home-a");
}

#[tokio::test]
async fn a_saved_rewrite_level_reaches_the_next_rewrite() {
    use kanade::chat::{
        nudge::{GovernedRewriter, NudgeRewriter, RewritePrompt},
        persona::{CompiledPersona, NudgeMood, PersonaId, parse_bundle},
    };

    let stub = Stub::start(gateway(listing(), "Right here, as always.")).await;
    let stack = stack(&stub, role("home-a", RoleEffort::Level(Effort::Low))).await;
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/config/personas/bundles/kanade.yaml"
    ))
    .unwrap();
    let bundle = parse_bundle(&text, &PersonaId::parse("kanade").unwrap()).unwrap();
    let prompt = RewritePrompt::build(
        &CompiledPersona::compile(&bundle, None),
        NudgeMood::Playful,
        "Everything you need is right here.",
    );
    let rewriter = GovernedRewriter::new(stack.client.clone());
    let deadline = Duration::from_secs(10);
    rewriter.rewrite(&prompt, deadline).await.unwrap();
    // Inherits extraction's off, which home-a allows.
    assert_eq!(last_sent(&stub), ("home-a".into(), Some("none".into())));

    let mut next = roles(role("home-a", RoleEffort::Level(Effort::Low)));
    next.rewrite = role("home-b", RoleEffort::Level(Effort::Medium));
    stack.apply_roles(next).unwrap();
    rewriter.rewrite(&prompt, deadline).await.unwrap();
    assert_eq!(last_sent(&stub), ("home-b".into(), Some("medium".into())));
}
