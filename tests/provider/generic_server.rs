//! A plain OpenAI-compatible server (no `kanata` metadata, a partial or
//! missing model list, OpenAI error envelopes) driven by operator
//! declarations; loopback stubs only. Kanata's own shapes stay pinned in
//! `http_shaping.rs`, `http_transport.rs` and `http_admission.rs`.

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use kanade::{
    domain::settings::ContextSettings,
    infrastructure::llm::{
        Capability, ChatRequest, Effort, ErrorCode, HttpProviderConfig, LlmProvider, Message,
        ModelCapabilities, NO_LISTING, OpenAiCompatibleProvider, OutputLimit, OutputSchema,
        OutputValidation, ProviderFailure, ProviderFailureKind, Sampling, ToolDefinition,
        governor::{QuestionLimits, Role, SessionFailure},
        setup::{ContextSource, Listing, ModelRoles, ModelStack, RoleEffort, resolve_context},
    },
};
use serde_json::{Value, json};

use super::{
    http_admission::client,
    http_transport::runner,
    setup::{ready, role, setup},
    stub::{Recorded, Reply, Stub, completion},
    support::tool_schema,
};

const ANSWER: &str = r#"{"answer":"ok"}"#;
const CHAT: &str = "plain/chat";
const EXTRACT: &str = "plain/extract";

/// A bare OpenAI listing: ids only, and not the declared models.
fn bare_listing() -> Value {
    json!({"object": "list", "data": [{"id": "other-model", "object": "model", "owned_by": "org"}]})
}

/// `GET /models` answers `listing` (status and body), completions echo the model.
fn server(listing: (u16, Value)) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |request| {
        if request.method == "GET" {
            Reply::Json(listing.0, listing.1.clone())
        } else {
            let model = request.body["model"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            Reply::Json(200, completion(&model, ANSWER))
        }
    }
}

fn declarations() -> BTreeMap<String, ModelCapabilities> {
    let chat = ModelCapabilities {
        structured_output: true,
        function_tools: true,
        sampling_controls: true,
        reasoning_control: true,
        reasoning_efforts: Some(vec![Effort::Low, Effort::High]),
        context_tokens: Some(32_768),
        max_output_tokens: Some(2_048),
        ..ModelCapabilities::minimal()
    };
    let extract = ModelCapabilities {
        structured_output: true,
        sampling_controls: true,
        context_tokens: Some(16_384),
        ..ModelCapabilities::minimal()
    };
    BTreeMap::from([(CHAT.to_owned(), chat), (EXTRACT.to_owned(), extract)])
}

fn stack(stub: &Stub) -> ModelStack {
    let mut input = setup(Some(format!("{}/v1", stub.url())));
    input.roles = ModelRoles {
        extraction: role(EXTRACT, RoleEffort::Level(Effort::Off)),
        chat: role(CHAT, RoleEffort::Level(Effort::High)),
        rewrite: Default::default(),
    };
    input.capabilities = declarations();
    ready(input)
}

/// Structured output, one tool and sampling: every optional field.
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
    let mut session = stack
        .client
        .open_question_on(
            &route,
            "member",
            false,
            QuestionLimits::new(Duration::from_secs(10)),
        )
        .await
        .unwrap();
    session
        .complete(&request(&route.alias, route.effort))
        .await
        .unwrap();
    stub.chat_requests().pop().unwrap().body
}

/// The catalog, context window and reserve clamp see a declared model the
/// listing omits; the request carries its declared fields.
fn assert_declared_models_apply(stack: &ModelStack, listed: bool) {
    let catalog = stack.catalog();
    assert_eq!(catalog.listed, listed);
    let chat = catalog
        .models
        .iter()
        .find(|model| model.alias == CHAT)
        .expect("declared model in the catalog");
    assert!(!chat.listed && !chat.published);
    assert!(chat.leaves_homelab, "a declared model is external");
    assert!(chat.function_tools && chat.structured_output);
    assert_eq!(
        (chat.context_tokens, chat.max_output_tokens),
        (Some(32_768), Some(2_048))
    );
    assert!(catalog.variant(CHAT).is_none());

    let mut settings = ContextSettings::default();
    settings.chat.reserve = 4_000;
    let resolved = resolve_context(&settings, &catalog, Role::Chat, CHAT);
    assert_eq!(resolved.source, ContextSource::Catalog);
    assert_eq!(
        resolved.window, 32_768,
        "the declared window, not cloud_default"
    );
    assert_eq!(resolved.reserve, 2_048, "clamped to the declared maximum");
    let extract = resolve_context(&settings, &catalog, Role::Extraction, EXTRACT);
    assert_eq!(
        (extract.window, extract.source),
        (16_384, ContextSource::Catalog)
    );
}

#[tokio::test]
async fn declared_models_apply_end_to_end_when_a_bare_listing_omits_them() {
    let stub = Stub::start(server((200, bare_listing()))).await;
    let stack = stack(&stub);
    // Before any listing the declarations are already in the catalog.
    assert_eq!(stack.catalog().models.len(), 2);

    let report = stack.check_startup().await;
    assert_eq!(report.listing, Listing::Listed { models: 1 });
    assert_declared_models_apply(&stack, true);
    let catalog = stack.catalog();
    let other = catalog
        .models
        .iter()
        .find(|model| model.alias == "other-model")
        .unwrap();
    assert!(other.listed && !other.published);
    assert_eq!(catalog.models.len(), 3);

    let body = ask(&stack, &stub).await;
    assert_eq!(body["model"], CHAT);
    assert_eq!(body["tools"][0]["function"]["name"], "tool");
    assert_eq!(body["response_format"]["type"], "json_schema");
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["temperature"], 0.25);
    assert_eq!(body["reasoning_effort"], "high");
    assert!(body.get("max_completion_tokens").is_none());
}

#[tokio::test]
async fn a_server_without_a_model_list_reports_no_listing_and_uses_declarations() {
    for status in [404, 405] {
        let stub = Stub::start(server((
            status,
            json!({"error": {"message": "Not Found", "type": "invalid_request_error"}}),
        )))
        .await;
        let stack = stack(&stub);
        assert_eq!(
            stack.provider.list_models().await.unwrap_err(),
            ProviderFailure {
                kind: ProviderFailureKind::Permanent,
                reason_code: NO_LISTING
            }
        );
        let report = stack.check_startup().await;
        assert_eq!(
            report.listing,
            Listing::Degraded {
                reason_code: "no-listing"
            },
            "{status}"
        );
        assert_declared_models_apply(&stack, false);
        assert_eq!(stack.catalog().models.len(), 2);

        let body = ask(&stack, &stub).await;
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["response_format"]["type"], "json_schema");
    }

    // A real outage stays distinct from "no list".
    let stub = Stub::start(server((500, json!({})))).await;
    let report = stack(&stub).check_startup().await;
    assert_eq!(
        report.listing,
        Listing::Degraded {
            reason_code: "server-error"
        }
    );
}

fn declared_provider(
    url: String,
    capabilities: ModelCapabilities,
) -> Arc<OpenAiCompatibleProvider> {
    let mut config = HttpProviderConfig::new(url);
    config.declared = BTreeMap::from([("qwen3:8b".to_owned(), capabilities)]);
    Arc::new(OpenAiCompatibleProvider::new(config).unwrap())
}

fn plain(url: String) -> Arc<OpenAiCompatibleProvider> {
    declared_provider(
        url,
        ModelCapabilities {
            structured_output: true,
            sampling_controls: true,
            reasoning_control: true,
            function_tools: true,
            ..ModelCapabilities::minimal()
        },
    )
}

fn plain_request() -> ChatRequest {
    let mut request = request("qwen3:8b", Some(Effort::Low));
    request.tools.clear();
    request
}

/// A strict server refusing an unknown field without naming it in `param`
/// is a permanent failure: one request, no downgrade, no loop.
#[tokio::test]
async fn a_strict_400_without_param_is_permanent_and_never_loops() {
    let rejecting = Stub::start(move |request: &Recorded| {
        if request.method == "GET" {
            return Reply::Json(200, bare_listing());
        }
        Reply::Json(
            400,
            json!({"error": {
                "message": "Unrecognized request argument supplied: seed",
                "type": "invalid_request_error",
                "param": null,
                "code": null
            }}),
        )
    })
    .await;
    let provider = plain(rejecting.url());
    assert_eq!(
        provider.complete(&plain_request()).await.unwrap_err(),
        ProviderFailure {
            kind: ProviderFailureKind::Permanent,
            reason_code: "status"
        }
    );
    let error = runner(provider.clone())
        .complete(&plain_request())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderPermanent);
    assert_eq!(
        rejecting.chat_requests().len(),
        2,
        "one per call, no resend"
    );
    let learned = provider.model_capabilities("qwen3:8b").await;
    assert!(learned.sampling_controls && learned.structured_output && learned.reasoning_control);
}

fn rejects(param: &'static str) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |request| {
        if request.method == "GET" {
            Reply::Json(200, bare_listing())
        } else if request.body.get(param).is_some() {
            Reply::Json(
                400,
                json!({"error": {
                    "message": format!("Unsupported parameter: '{param}'"),
                    "type": "invalid_request_error",
                    "param": param,
                    "code": "unsupported_parameter"
                }}),
            )
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    }
}

#[tokio::test]
async fn output_limit_sends_max_completion_tokens_without_sampling() {
    let reasoning_model = ModelCapabilities {
        structured_output: true,
        reasoning_control: true,
        output_limit: Some(OutputLimit::MaxCompletionTokens),
        ..ModelCapabilities::minimal()
    };
    let stub = Stub::start(server((200, bare_listing()))).await;
    runner(declared_provider(stub.url(), reasoning_model.clone()))
        .complete(&plain_request())
        .await
        .unwrap();
    let body = stub.chat_requests()[0].body.clone();
    assert_eq!(body["max_completion_tokens"], 64);
    for absent in ["max_tokens", "temperature", "seed", "top_p"] {
        assert!(body.get(absent).is_none(), "{absent}");
    }
    assert_eq!(body["reasoning_effort"], "low");

    // A 400 naming the declared field drops only the output limit.
    let stub = Stub::start(rejects("max_completion_tokens")).await;
    let provider = declared_provider(stub.url(), reasoning_model);
    runner(provider.clone())
        .complete(&plain_request())
        .await
        .unwrap();
    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 2);
    assert_eq!(chats[0].body["max_completion_tokens"], 64);
    assert!(chats[1].body.get("max_completion_tokens").is_none());
    assert!(chats[1].body.get("max_tokens").is_none());
    assert_eq!(chats[1].body["response_format"]["type"], "json_schema");
    let learned = provider.model_capabilities("qwen3:8b").await;
    assert_eq!(learned.output_limit, Some(OutputLimit::Omitted));
    assert!(learned.structured_output && learned.reasoning_control);
    // Remembered: the next call sends no size field at once.
    runner(provider).complete(&plain_request()).await.unwrap();
    assert_eq!(stub.chat_requests().len(), 3);
}

#[tokio::test]
async fn a_declared_max_tokens_limit_is_independent_of_sampling() {
    let sized = ModelCapabilities {
        sampling_controls: true,
        structured_output: true,
        output_limit: Some(OutputLimit::MaxTokens),
        ..ModelCapabilities::minimal()
    };
    let stub = Stub::start(rejects("max_tokens")).await;
    let provider = declared_provider(stub.url(), sized);
    let direct = provider.complete(&plain_request()).await.unwrap_err();
    assert_eq!(
        direct.kind,
        ProviderFailureKind::CapabilityRejected(Capability::OutputLimit)
    );
    let learned = provider.model_capabilities("qwen3:8b").await;
    assert_eq!(learned.output_limit, Some(OutputLimit::Omitted));
    assert!(learned.sampling_controls, "sampling survives");
    runner(provider).complete(&plain_request()).await.unwrap();
    let last = stub.chat_requests().pop().unwrap().body;
    assert!(last.get("max_tokens").is_none());
    assert_eq!(last["temperature"], 0.25);

    // Without sampling controls a declared `max_tokens` is still sent.
    let stub = Stub::start(server((200, bare_listing()))).await;
    let bare = ModelCapabilities {
        output_limit: Some(OutputLimit::MaxTokens),
        ..ModelCapabilities::minimal()
    };
    let mut request = plain_request();
    request.output_schema = None;
    runner(declared_provider(stub.url(), bare))
        .complete(&request)
        .await
        .unwrap();
    let body = stub.chat_requests()[0].body.clone();
    assert_eq!(body["max_tokens"], 64);
    assert!(body.get("temperature").is_none());
}

/// OpenAI's full error envelope for an exhausted account: permanent, a
/// misconfiguration, never retried.
#[tokio::test]
async fn insufficient_quota_is_a_permanent_misconfiguration() {
    let quota = || {
        Reply::Json(
            429,
            json!({"error": {
                "message": "You exceeded your current quota, please check your plan and billing details.",
                "type": "insufficient_quota",
                "param": null,
                "code": "insufficient_quota"
            }}),
        )
    };
    let stub = Stub::start(move |request: &Recorded| {
        if request.method == "GET" {
            Reply::Json(200, bare_listing())
        } else {
            quota()
        }
    })
    .await;
    let provider = plain(stub.url());
    assert_eq!(
        provider.complete(&plain_request()).await.unwrap_err(),
        ProviderFailure {
            kind: ProviderFailureKind::Permanent,
            reason_code: "quota"
        }
    );
    let error = runner(provider.clone())
        .complete(&plain_request())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::QuotaExhausted);
    assert_eq!(stub.chat_requests().len(), 2, "never retried");

    let client = client(provider);
    let mut extraction = client
        .open_extraction("run", Duration::from_secs(5), Duration::from_secs(30))
        .await
        .unwrap();
    let failed = extraction.complete(&plain_request()).await.unwrap_err();
    assert!(matches!(
        &failed.failure,
        SessionFailure::Model(error) if error.code == ErrorCode::QuotaExhausted
    ));
    assert!(failed.is_misconfiguration());
    assert_eq!(failed.code(), "quota_exhausted");
    assert_eq!(stub.chat_requests().len(), 3);
}
