use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};

use kanade::infrastructure::llm::{
    BearerKey, ChatRequest, CompletionRunner, ErrorCode, ExecutionLimits, HttpConfigError,
    HttpLimits, HttpProviderConfig, LlmProvider, ModelCapabilities, OpenAiCompatibleProvider,
    OutputSchema, OutputValidation, ProviderFailure, ProviderFailureKind, Sampling, TrustRoots,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::{Value, json};
use tokio_rustls::TlsAcceptor;

use super::{
    stub::{Recorded, Reply, Stub, completion, gateway, ollama_models},
    support::{default_retry, tiny_request},
};

const ANSWER: &str = r#"{"answer":"ok"}"#;
const KEY: &str = "sk-provider-secret-sentinel";

pub(super) fn structured() -> ChatRequest {
    let mut request = tiny_request();
    request.model = "qwen3:8b".into();
    request.max_output_tokens = 32;
    request.output_schema = Some(OutputSchema {
        name: "answer".into(),
        schema: json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        }),
        strict: true,
        validation: OutputValidation::Runner,
    });
    request.sampling = Some(Sampling {
        temperature: Some(0.0),
        seed: Some(1),
        top_p: None,
    });
    request
}

pub(super) fn full_capabilities() -> BTreeMap<String, ModelCapabilities> {
    BTreeMap::from([(
        "qwen3:8b".to_owned(),
        ModelCapabilities {
            structured_output: true,
            sampling_controls: true,
            reasoning_control: true,
            function_tools: true,
            ..ModelCapabilities::minimal()
        },
    )])
}

pub(super) fn build(config: HttpProviderConfig) -> Arc<OpenAiCompatibleProvider> {
    Arc::new(OpenAiCompatibleProvider::new(config).unwrap())
}

pub(super) fn declared(url: String) -> Arc<OpenAiCompatibleProvider> {
    let mut config = HttpProviderConfig::new(url);
    config.declared = full_capabilities();
    build(config)
}

pub(super) fn runner(
    provider: Arc<OpenAiCompatibleProvider>,
) -> CompletionRunner<OpenAiCompatibleProvider> {
    CompletionRunner::ungoverned(provider, ExecutionLimits::default(), default_retry()).unwrap()
}

pub(super) fn models_or(
    reply: impl Fn(&Recorded) -> Reply + Send + Sync + 'static,
) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    move |request| {
        if request.method == "GET" {
            Reply::Json(200, ollama_models())
        } else {
            reply(request)
        }
    }
}

fn rejects(param: &'static str) -> impl Fn(&Recorded) -> Reply + Send + Sync + 'static {
    models_or(move |request| {
        if request.body.get(param).is_some() {
            Reply::Json(
                400,
                json!({"error": {"message": "unsupported", "type": "invalid_request_error", "param": param}}),
            )
        } else if request.body.get("response_format").is_none() && !has_schema_instruction(request)
        {
            Reply::Json(
                422,
                json!({"error": {"message": "schema instruction missing"}}),
            )
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    })
}

fn has_schema_instruction(request: &Recorded) -> bool {
    request.body["messages"][0]["role"] == "system"
        && request.body["messages"][0]["content"]
            .as_str()
            .is_some_and(|content| content.starts_with("OUTPUT FORMAT\n"))
}

#[tokio::test]
async fn rejected_optional_field_retries_once_and_is_remembered() {
    let stub = Stub::start(rejects("response_format")).await;
    let provider = declared(stub.url());
    let runner = runner(provider.clone());

    runner.complete(&structured()).await.unwrap();
    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 2);
    assert!(chats[0].body.get("response_format").is_some());
    assert!(!has_schema_instruction(&chats[0]));
    assert!(chats[1].body.get("response_format").is_none());
    assert!(
        has_schema_instruction(&chats[1]),
        "retry carries the schema"
    );
    assert_eq!(chats[1].body["temperature"], 0.0, "other capabilities stay");

    runner.complete(&structured()).await.unwrap();
    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 3);
    assert!(chats[2].body.get("response_format").is_none());
    let system = chats[2].body["messages"][0]["content"].as_str().unwrap();
    assert!(
        system.starts_with("OUTPUT FORMAT\n"),
        "downgrade shapes later calls"
    );
    assert!(
        !provider
            .model_capabilities("qwen3:8b")
            .await
            .structured_output
    );
}

#[tokio::test]
async fn rejected_sampling_field_drops_the_whole_capability() {
    let stub = Stub::start(rejects("seed")).await;
    let provider = declared(stub.url());
    let runner = runner(provider);
    runner.complete(&structured()).await.unwrap();
    runner.complete(&structured()).await.unwrap();
    let chats = stub.chat_requests();
    assert_eq!(chats.len(), 3);
    for field in ["max_tokens", "temperature", "seed"] {
        assert!(chats[0].body.get(field).is_some(), "{field}");
        assert!(chats[1].body.get(field).is_none(), "{field}");
        assert!(chats[2].body.get(field).is_none(), "{field}");
    }
    assert!(chats[1].body.get("response_format").is_some());
}

#[tokio::test]
async fn unknown_unsent_or_repeated_rejections_do_not_loop() {
    let error = |param: &str| json!({"error": {"message": "bad", "param": param}});
    for (param, sent) in [("messages", true), ("reasoning_effort", false)] {
        let body = error(param);
        let stub = Stub::start(models_or(move |_| Reply::Json(400, body.clone()))).await;
        let provider = declared(stub.url());
        assert_eq!(
            runner(provider.clone())
                .complete(&structured())
                .await
                .unwrap_err()
                .code,
            ErrorCode::ProviderPermanent,
            "{param}"
        );
        assert_eq!(stub.chat_requests().len(), 1, "{param} sent={sent}");
        assert!(
            provider
                .model_capabilities("qwen3:8b")
                .await
                .reasoning_control
        );
    }

    let stub = Stub::start(models_or(move |request| {
        let param = if request.body.get("response_format").is_some() {
            "response_format"
        } else {
            "temperature"
        };
        Reply::Json(400, json!({"error": {"param": param}}))
    }))
    .await;
    let provider = declared(stub.url());
    assert_eq!(
        runner(provider.clone())
            .complete(&structured())
            .await
            .unwrap_err()
            .code,
        ErrorCode::ProviderPermanent
    );
    assert_eq!(stub.chat_requests().len(), 2);
    let learned = provider.model_capabilities("qwen3:8b").await;
    assert!(!learned.structured_output && !learned.sampling_controls);
}

#[tokio::test]
async fn statuses_map_to_provider_failure_kinds() {
    for (status, kind) in [
        (401, ProviderFailureKind::Authentication),
        (403, ProviderFailureKind::Authentication),
        (429, ProviderFailureKind::Transient),
        (500, ProviderFailureKind::Transient),
        (502, ProviderFailureKind::Transient),
        (503, ProviderFailureKind::Transient),
        (504, ProviderFailureKind::UpstreamTimeout),
        (400, ProviderFailureKind::Permanent),
        (404, ProviderFailureKind::Permanent),
        (422, ProviderFailureKind::Permanent),
    ] {
        let stub = Stub::start(models_or(move |_| {
            Reply::Json(status, json!({"error": {"message": KEY}}))
        }))
        .await;
        let provider = declared(stub.url());
        let failure = provider.complete(&structured()).await.unwrap_err();
        assert_eq!(failure.kind, kind, "{status}");
        assert!(!format!("{failure:?}").contains(KEY));
    }

    let stub = Stub::start(models_or(|_| Reply::Json(401, json!({})))).await;
    let error = runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderAuthentication);
    assert_eq!(stub.chat_requests().len(), 1);

    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = attempts.clone();
    let stub = Stub::start(models_or(move |_| {
        if seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            Reply::Json(429, json!({}))
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    }))
    .await;
    runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap();
    assert_eq!(stub.chat_requests().len(), 2, "429 retried by the runner");

    let stub = Stub::start(models_or(|_| Reply::Json(502, json!({})))).await;
    assert_eq!(
        runner(declared(stub.url()))
            .complete(&structured())
            .await
            .unwrap_err()
            .code,
        ErrorCode::ProviderPermanent
    );
    assert_eq!(
        stub.chat_requests().len(),
        3,
        "5xx exhausts the retry policy"
    );
}

#[test]
fn plain_http_is_rejected_for_non_local_hosts() {
    for url in [
        "http://10.0.0.5:11434/v1",
        "http://gw.example",
        "http://192.168.0.1",
    ] {
        assert_eq!(
            OpenAiCompatibleProvider::new(HttpProviderConfig::new(url)).unwrap_err(),
            HttpConfigError::InsecureHttp
        );
    }
    for url in [
        "http://127.0.0.1:11434/v1",
        "http://localhost:11434",
        "http://[::1]:11434",
        "http://host.docker.internal:11434/v1",
    ] {
        assert!(OpenAiCompatibleProvider::new(HttpProviderConfig::new(url)).is_ok());
    }
}

#[tokio::test]
async fn bearer_key_is_sent_only_when_configured_and_never_printed() {
    assert_eq!(
        BearerKey::from_bytes(b" \n").unwrap_err(),
        HttpConfigError::InvalidBearerKey
    );
    assert_eq!(
        BearerKey::from_bytes(b"has space").unwrap_err(),
        HttpConfigError::InvalidBearerKey
    );

    let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
    let mut config = HttpProviderConfig::new(stub.url());
    config.declared = full_capabilities();
    config.bearer_key = Some(BearerKey::from_bytes(format!("{KEY}\n").as_bytes()).unwrap());
    let config_debug = format!("{config:?}");
    let provider = build(config);
    runner(provider.clone())
        .complete(&structured())
        .await
        .unwrap();
    let requests = stub.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            request.header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
    }
    for text in [config_debug, format!("{provider:?}")] {
        assert!(!text.contains(KEY), "{text}");
    }

    let stub = Stub::start(models_or(|_| Reply::Json(401, json!({})))).await;
    let mut config = HttpProviderConfig::new(stub.url());
    config.bearer_key = Some(BearerKey::from_bytes(KEY.as_bytes()).unwrap());
    let provider = build(config);
    let failure = provider.complete(&structured()).await.unwrap_err();
    let error = runner(provider).complete(&structured()).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderAuthentication);
    for text in [
        format!("{failure:?}"),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(!text.contains(KEY), "{text}");
    }

    let stub = Stub::start(gateway(ollama_models(), ANSWER)).await;
    runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap();
    assert!(
        stub.requests()
            .iter()
            .all(|request| request.header("authorization").is_none())
    );
}

#[tokio::test]
async fn per_request_timeout_is_honoured() {
    let stub = Stub::start(models_or(|_| Reply::Hang)).await;
    let mut config = HttpProviderConfig::new(stub.url());
    config.declared = full_capabilities();
    config.timeout = Duration::from_millis(200);
    let provider = build(config);
    let started = Instant::now();
    let failure = provider.complete(&structured()).await.unwrap_err();
    assert_eq!(
        failure,
        ProviderFailure {
            kind: ProviderFailureKind::UpstreamTimeout,
            reason_code: "timeout"
        }
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn oversized_bodies_are_rejected() {
    let content = "x".repeat(2048);
    let body = completion("qwen3:8b", &content).to_string();
    let known = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
        body.len()
    );
    for raw in [known, chunked] {
        let stub = Stub::start(models_or(move |_| Reply::Raw(raw.clone().into_bytes()))).await;
        let mut config = HttpProviderConfig::new(stub.url());
        config.declared = full_capabilities();
        config.limits = HttpLimits {
            completion_body_bytes: 1024,
            ..HttpLimits::default()
        };
        let failure = build(config).complete(&structured()).await.unwrap_err();
        assert_eq!(
            failure,
            ProviderFailure {
                kind: ProviderFailureKind::InvalidOutput,
                reason_code: "body-size"
            }
        );
    }

    let listing = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: 300000\r\n\r\n{}",
        " ".repeat(300_000)
    );
    let stub = Stub::start(move |_| Reply::Raw(listing.clone().into_bytes())).await;
    let provider = build(HttpProviderConfig::new(stub.url()));
    assert_eq!(
        provider.list_models().await.unwrap_err().kind,
        ProviderFailureKind::InvalidOutput
    );
    assert_eq!(
        provider.model_capabilities("anything").await,
        ModelCapabilities::minimal()
    );
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let target = Stub::start(gateway(ollama_models(), ANSWER)).await;
    let location = format!("{}/v1/chat/completions", target.url());
    let stub = Stub::start(models_or(move |_| {
        Reply::Raw(
            format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n")
                .into_bytes(),
        )
    }))
    .await;
    let failure = declared(stub.url())
        .complete(&structured())
        .await
        .unwrap_err();
    assert_eq!(
        failure,
        ProviderFailure {
            kind: ProviderFailureKind::Permanent,
            reason_code: "redirect"
        }
    );
    assert_eq!(stub.chat_requests().len(), 1);
    assert!(target.requests().is_empty());
}

fn tls_acceptor() -> (TlsAcceptor, CertificateDer<'static>) {
    let _ = kanade::runtime::tls::install_ring_provider();
    let cert =
        CertificateDer::from(include_bytes!("../fixtures/provider/tls/loopback-cert.der").to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        include_bytes!("../fixtures/provider/tls/loopback-key.pk8.der").to_vec(),
    ));
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.clone()], key)
        .unwrap();
    (TlsAcceptor::from(Arc::new(config)), cert)
}

#[tokio::test]
async fn https_verifies_the_server_against_the_configured_roots() {
    let (acceptor, cert) = tls_acceptor();
    let stub = Stub::start_with(gateway(ollama_models(), ANSWER), Some(acceptor)).await;
    for host in ["127.0.0.1", "localhost"] {
        let mut config = HttpProviderConfig::new(format!("https://{host}:{}/v1", stub.addr.port()));
        config.declared = full_capabilities();
        config.trust_roots = TrustRoots::Custom(vec![cert.clone()]);
        runner(build(config)).complete(&structured()).await.unwrap();
    }
    assert_eq!(stub.chat_requests().len(), 2);

    let mut config = HttpProviderConfig::new(format!("https://127.0.0.1:{}", stub.addr.port()));
    config.declared = full_capabilities();
    let failure = build(config).complete(&structured()).await.unwrap_err();
    assert_eq!(
        failure,
        ProviderFailure {
            kind: ProviderFailureKind::Permanent,
            reason_code: "tls"
        }
    );
    assert_eq!(
        stub.chat_requests().len(),
        2,
        "unverified server saw nothing"
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    let mut config = HttpProviderConfig::new(format!("https://127.0.0.1:{port}"));
    config.declared = full_capabilities();
    config.trust_roots = TrustRoots::Custom(vec![cert.clone()]);
    assert_eq!(
        build(config).complete(&structured()).await.unwrap_err(),
        ProviderFailure {
            kind: ProviderFailureKind::Transient,
            reason_code: "tls-io"
        },
        "EOF during the handshake is transient"
    );

    let mut config = HttpProviderConfig::new("https://127.0.0.1:1");
    config.trust_roots = TrustRoots::Custom(vec![CertificateDer::from(vec![0u8; 4])]);
    assert_eq!(
        OpenAiCompatibleProvider::new(config).unwrap_err(),
        HttpConfigError::InvalidTrustRoots
    );
}

#[tokio::test]
async fn completion_wire_replies_are_normalized() {
    let stub = Stub::start(models_or(|_| {
        Reply::Json(
            200,
            json!({
                "choices": [{
                    "message": {
                        "content": null,
                        "tool_calls": [
                            {"type": "function", "function": {"name": "tool", "arguments": {"value": "ok"}}}
                        ]
                    },
                    "finish_reason": "stop"
                }]
            }),
        )
    }))
    .await;
    let provider = declared(stub.url());
    let mut request = super::support::tool_request();
    request.model = "qwen3:8b".into();
    let completed = runner(provider).complete(&request).await.unwrap();
    assert_eq!(completed.model, "qwen3:8b");
    assert_eq!(completed.tool_calls[0].id, "call_0");
    assert_eq!(
        serde_json::from_str::<Value>(&completed.tool_calls[0].arguments).unwrap(),
        json!({"value": "ok"})
    );
    assert!(completed.usage.is_none());
}
