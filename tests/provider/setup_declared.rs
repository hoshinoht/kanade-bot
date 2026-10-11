//! `[models.capabilities]` in `kanade.toml` reaches the running stack: a plain
//! OpenAI-compatible listing gets the declared request fields, efforts and
//! catalog view, and Kanata's published metadata still wins for an alias it
//! describes.

use std::{collections::BTreeMap, time::Duration};

use kanade::infrastructure::llm::{
    ChatRequest, Effort, Message, ModelCapabilities, OutputSchema, OutputValidation, Sampling,
    ToolDefinition,
    governor::{QuestionLimits, Role},
    parse_models_list,
    setup::{EffortStatus, ModelRoles, ModelStack, RoleEffort, RoleModel},
};
use kanade::runtime::config::{ModelSettings, resolve};
use serde_json::{Value, json};

use super::setup::{ready, role, setup, temp_file};
use super::stub::{Stub, gateway, kanata_models};
use super::support::tool_schema;

const ANSWER: &str = r#"{"answer":"ok"}"#;

/// Model settings read the way serve reads them: `KANADE_CONFIG` → TOML.
fn settings(base_url: &str, tables: &str) -> ModelSettings {
    let path = temp_file(format!("[models]\nbase_url = \"{base_url}\"\n\n{tables}").as_bytes());
    let environment = BTreeMap::from([("KANADE_CONFIG".to_owned(), path.display().to_string())]);
    let resolved = resolve(environment);
    std::fs::remove_file(&path).unwrap();
    let resolved = resolved.unwrap();
    ModelSettings::from_mapping(&resolved.values)
        .map_err(|error| resolved.annotate(error).to_string())
        .unwrap()
}

fn stack(settings: ModelSettings, roles: ModelRoles) -> ModelStack {
    let mut input = setup(settings.base_url.clone());
    input.roles = roles;
    input.capabilities = settings.capabilities;
    ready(input)
}

/// Structured output, one tool and sampling: every optional field a
/// capability can switch on.
fn request(alias: &str, effort: Option<Effort>) -> ChatRequest {
    ChatRequest {
        model: alias.into(),
        messages: vec![Message::User {
            content: "hi".into(),
        }],
        tools: vec![ToolDefinition {
            name: "tool".into(),
            description: Some("synthetic".into()),
            input_schema: tool_schema(),
        }],
        output_schema: Some(OutputSchema {
            name: "answer".into(),
            schema: json!({
                "type": "object",
                "properties": {"answer": {"type": "string"}},
                "required": ["answer"],
                "additionalProperties": false
            }),
            strict: true,
            validation: OutputValidation::Runner,
        }),
        max_output_tokens: 64,
        reasoning: effort,
        sampling: Some(Sampling {
            temperature: Some(0.25),
            seed: Some(7),
            top_p: Some(0.5),
        }),
    }
}

/// One governed chat question on the running route; returns the wire body.
async fn ask(stack: &ModelStack, stub: &Stub) -> Value {
    let route = stack.governor.route(Role::Chat).unwrap();
    let limits = QuestionLimits::new(Duration::from_secs(10));
    let mut session = stack
        .client
        .open_question_on(&route, "member", false, limits)
        .await
        .unwrap();
    session
        .complete(&request(&route.alias, route.effort))
        .await
        .unwrap();
    stub.chat_requests().pop().unwrap().body
}

fn on(alias: &str, chat: RoleEffort) -> ModelRoles {
    ModelRoles {
        extraction: role(alias, RoleEffort::Level(Effort::High)),
        chat: role(alias, chat),
        rewrite: RoleModel::default(),
    }
}

#[tokio::test]
async fn a_declared_alias_gets_its_toml_capabilities_end_to_end() {
    let listing = json!({"object": "list", "data": [
        {"id": "plain-model", "object": "model"},
        {"id": "other-model", "object": "model"}
    ]});
    let stub = Stub::start(gateway(listing, ANSWER)).await;
    let settings = settings(
        &format!("{}/v1", stub.url()),
        r#"[models.capabilities."plain-model"]
structured_output = true
function_tools = true
sampling_controls = true
reasoning_control = true
reasoning_efforts = ["low", "high"]
context_tokens = 32768
max_output_tokens = 4096
"#,
    );
    let declared = ModelCapabilities {
        structured_output: true,
        function_tools: true,
        sampling_controls: true,
        reasoning_control: true,
        reasoning_efforts: Some(vec![Effort::Low, Effort::High]),
        context_tokens: Some(32_768),
        max_output_tokens: Some(4096),
        ..ModelCapabilities::minimal()
    };
    assert_eq!(settings.capabilities["plain-model"], declared);
    // Chat asks for `medium`, which the declared list does not accept.
    let stack = stack(
        settings,
        on("plain-model", RoleEffort::Level(Effort::Medium)),
    );
    let listed = stack.provider.list_models().await.unwrap();
    assert!(listed.iter().all(|model| model.capabilities.is_none()));

    assert_eq!(
        stack.provider.model_capabilities("plain-model").await,
        declared
    );
    assert_eq!(
        stack.provider.model_capabilities("other-model").await,
        ModelCapabilities::minimal()
    );
    let catalog = stack.catalog();
    let shown = catalog
        .models
        .iter()
        .find(|model| model.alias == "plain-model")
        .unwrap();
    assert!(!shown.published, "declared, not published");
    assert!(shown.leaves_homelab, "a declaration never sets a home zone");
    assert!(shown.function_tools && shown.structured_output && shown.sampling_controls);
    assert_eq!(
        shown.reasoning_efforts,
        Some(vec![Effort::Low, Effort::High])
    );
    assert_eq!(
        (shown.context_tokens, shown.max_output_tokens),
        (Some(32_768), Some(4096))
    );
    let other = catalog
        .models
        .iter()
        .find(|model| model.alias == "other-model")
        .unwrap();
    assert!(!other.function_tools && !other.structured_output);

    let efforts = stack.efforts();
    assert_eq!(
        efforts[&Role::Extraction],
        EffortStatus {
            effort: Effort::High,
            stranded: None
        }
    );
    assert_eq!(
        efforts[&Role::Chat],
        EffortStatus {
            effort: Effort::Low,
            stranded: Some(Effort::Medium)
        },
        "a stranded level falls to the lowest declared one"
    );

    let body = ask(&stack, &stub).await;
    assert_eq!(body["model"], "plain-model");
    assert_eq!(body["tools"][0]["function"]["name"], "tool");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(
        (&body["temperature"], &body["seed"], &body["top_p"]),
        (&json!(0.25), &json!(7), &json!(0.5))
    );
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["reasoning_effort"], "low");
}

#[tokio::test]
async fn kanata_published_metadata_overrides_a_declaration() {
    let stub = Stub::start(gateway(kanata_models(), ANSWER)).await;
    // Declarations that contradict what Kanata publishes for `sumi-structured`;
    // `plain-alias` is listed without metadata, so its declaration applies.
    let settings = settings(
        &stub.url(),
        r#"[models.capabilities."sumi-structured"]
structured_output = false
function_tools = false
sampling_controls = false
reasoning_control = false
context_tokens = 1024

[models.capabilities."plain-alias"]
structured_output = true
"#,
    );
    let stack = stack(
        settings,
        on("sumi-structured", RoleEffort::Level(Effort::Low)),
    );
    stack.provider.list_models().await.unwrap();

    let published = parse_models_list(&serde_json::to_vec(&kanata_models()).unwrap())
        .unwrap()
        .into_iter()
        .find(|model| model.id == "sumi-structured")
        .and_then(|model| model.capabilities)
        .unwrap();
    assert_eq!(
        stack.provider.model_capabilities("sumi-structured").await,
        published
    );
    assert_eq!(
        stack.provider.model_capabilities("plain-alias").await,
        ModelCapabilities {
            structured_output: true,
            ..ModelCapabilities::minimal()
        }
    );
    let catalog = stack.catalog();
    let shown = catalog
        .models
        .iter()
        .find(|model| model.alias == "sumi-structured")
        .unwrap();
    assert!(shown.published && shown.structured_output && shown.function_tools);
    assert_eq!(shown.context_tokens, Some(32_768));

    let body = ask(&stack, &stub).await;
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["tools"][0]["function"]["name"], "tool");
    assert_eq!(body["temperature"], 0.25);
    assert_eq!(body["reasoning_effort"], "low");
}
