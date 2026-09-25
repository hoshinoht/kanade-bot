//! Failure classification for gateway admission, backend-down and timeout
//! replies. The Kanata codes are provisional (not shipped yet); loopback stubs only.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use kanade::infrastructure::llm::{
    ErrorCode, ExecutionLimits, LlmProvider, OpenAiCompatibleProvider, ProviderFailure,
    ProviderFailureKind,
    governor::{
        Charge, Governor, GovernorConfig, GovernorPolicy, GroupConfig, ModelClient, QuestionLimits,
        Role, RoleConfig, SessionFailure, XorShift,
    },
};
use serde_json::json;

use super::{
    http_transport::{declared, models_or, runner, structured},
    stub::{Reply, Stub, completion},
    support::default_retry,
};

const ANSWER: &str = r#"{"answer":"ok"}"#;

fn raw(status: u16, code: Option<&str>, retry_after: Option<&str>) -> Vec<u8> {
    let body = match code {
        Some(code) => json!({"error": {"message": "busy", "type": "gateway", "code": code}}),
        None => json!({"error": {"message": "busy"}}),
    }
    .to_string();
    let header = retry_after
        .map(|value| format!("Retry-After: {value}\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 {status} Stub\r\nContent-Type: application/json\r\n{header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn classify(
    status: u16,
    code: Option<&'static str>,
    retry_after: Option<&'static str>,
) -> ProviderFailureKind {
    let stub = Stub::start(models_or(move |_| {
        Reply::Raw(raw(status, code, retry_after))
    }))
    .await;
    declared(stub.url())
        .complete(&structured())
        .await
        .unwrap_err()
        .kind
}

#[tokio::test]
async fn provisional_kanata_admission_codes_are_admission_refusals() {
    for (status, code) in [
        (429, "gateway_queue_full"),
        (429, "gateway_key_busy"),
        (429, "gateway_key_rate_limited"),
        (503, "gateway_busy"),
    ] {
        assert_eq!(
            classify(status, Some(code), Some("3")).await,
            ProviderFailureKind::AdmissionRefused {
                retry_after: Some(Duration::from_secs(3))
            },
            "{status} {code}"
        );
    }
    assert_eq!(
        classify(429, Some("gateway_queue_full"), None).await,
        ProviderFailureKind::AdmissionRefused { retry_after: None }
    );
    assert_eq!(
        classify(
            503,
            Some("gateway_busy"),
            Some("Wed, 21 Oct 2026 07:28:00 GMT")
        )
        .await,
        ProviderFailureKind::AdmissionRefused { retry_after: None },
        "HTTP-date Retry-After is ignored"
    );
    assert_eq!(
        classify(429, Some("gateway_busy"), Some("999999999999")).await,
        ProviderFailureKind::AdmissionRefused {
            retry_after: Some(Duration::from_secs(3_600))
        },
        "Retry-After is capped"
    );
}

#[tokio::test]
async fn backend_down_timeouts_and_generic_statuses_are_classified() {
    for (status, code, kind) in [
        (
            503,
            Some("upstream_unavailable"),
            ProviderFailureKind::BackendUnavailable,
        ),
        (
            504,
            Some("upstream_timeout"),
            ProviderFailureKind::UpstreamTimeout,
        ),
        (504, None, ProviderFailureKind::UpstreamTimeout),
        (
            429,
            Some("rate_limit_exceeded"),
            ProviderFailureKind::RateLimited {
                retry_after: Duration::from_secs(5),
            },
        ),
        (
            429,
            None,
            ProviderFailureKind::RateLimited {
                retry_after: Duration::from_secs(5),
            },
        ),
        (503, None, ProviderFailureKind::Transient),
        // Admission codes count only on 429/503.
        (
            400,
            Some("gateway_queue_full"),
            ProviderFailureKind::Permanent,
        ),
    ] {
        assert_eq!(
            classify(status, code, Some("5")).await,
            kind,
            "{status} {code:?}"
        );
    }
}

#[tokio::test]
async fn the_runner_never_retries_admission_refusals_or_a_down_backend() {
    for (code, expected) in [
        ("gateway_queue_full", ErrorCode::AdmissionRefused),
        ("upstream_unavailable", ErrorCode::BackendUnavailable),
    ] {
        let stub = Stub::start(models_or(move |_| {
            Reply::Raw(raw(503, Some(code), Some("1")))
        }))
        .await;
        let error = runner(declared(stub.url()))
            .complete(&structured())
            .await
            .unwrap_err();
        assert_eq!(error.code, expected, "{code}");
        assert_eq!(stub.chat_requests().len(), 1, "{code}");
    }
}

#[tokio::test]
async fn a_plain_429_retry_after_floors_the_backoff() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let stub = Stub::start(models_or(move |_| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            Reply::Raw(raw(429, None, Some("1")))
        } else {
            Reply::Json(200, completion("qwen3:8b", ANSWER))
        }
    }))
    .await;
    let started = std::time::Instant::now();
    runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_secs(1));
    assert_eq!(stub.chat_requests().len(), 2);
}

/// A 200 whose body stops short, and a connection closed without any reply.
fn lost_replies() -> [Vec<u8>; 2] {
    [
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 500\r\n\r\n{\"choices\""
            .to_vec(),
        Vec::new(),
    ]
}

#[tokio::test]
async fn a_reply_lost_after_sending_is_an_upstream_timeout() {
    for lost in lost_replies() {
        let stub = Stub::start(models_or(move |_| Reply::Raw(lost.clone()))).await;
        assert_eq!(
            declared(stub.url())
                .complete(&structured())
                .await
                .unwrap_err(),
            ProviderFailure {
                kind: ProviderFailureKind::UpstreamTimeout,
                reason_code: "interrupted"
            }
        );
    }
}

fn client(provider: Arc<OpenAiCompatibleProvider>) -> ModelClient<OpenAiCompatibleProvider> {
    let roles = [Role::Chat, Role::Extraction]
        .into_iter()
        .map(|role| {
            let config = RoleConfig {
                alias: "qwen3:8b".into(),
                external: false,
            };
            (role, config)
        })
        .collect::<BTreeMap<_, _>>();
    let config = GovernorConfig {
        groups: vec![GroupConfig {
            name: "local".into(),
            backend: "stub".into(),
            permits: 1,
            requests_per_min: 6_000,
            burst: Some(100),
            aliases: vec!["qwen3:8b".into()],
        }],
        roles,
        policy: GovernorPolicy {
            retry_floor: 10,
            ..GovernorPolicy::default()
        },
    };
    let governor = Governor::new(&config, Arc::new(XorShift::new(7))).unwrap();
    ModelClient::new(
        Arc::new(governor),
        provider,
        ExecutionLimits::default(),
        default_retry(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_lost_chat_reply_is_charged_and_never_retried() {
    for lost in lost_replies() {
        let stub = Stub::start(models_or(move |_| Reply::Raw(lost.clone()))).await;
        let client = client(declared(stub.url()));
        let limits = QuestionLimits::new(Duration::from_secs(30));
        let mut question = client.open_question("member", false, limits).await.unwrap();
        let error = question.complete(&structured()).await.unwrap_err();
        assert!(matches!(
            &error.failure,
            SessionFailure::Model(model) if model.code == ErrorCode::UpstreamTimeout
        ));
        assert_eq!(error.charge, Charge::Charged);
        assert!(question.is_ended());
        assert_eq!(stub.chat_requests().len(), 1);

        drop(question);

        // Extraction may retry within its budget.
        let mut extraction = client
            .open_extraction("run", Duration::from_secs(5), Duration::from_secs(30))
            .await
            .unwrap();
        extraction.complete(&structured()).await.unwrap_err();
        assert_eq!(stub.chat_requests().len(), 4);
    }
}
