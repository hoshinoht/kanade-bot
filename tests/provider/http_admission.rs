//! Failure classification for gateway admission, backend-down and timeout
//! replies, pinned to the codes and statuses Kanata sends; loopback stubs only.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use kanade_llm::{
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
) -> (ProviderFailureKind, &'static str) {
    let stub = Stub::start(models_or(move |_| {
        Reply::Raw(raw(status, code, retry_after))
    }))
    .await;
    let failure = declared(stub.url())
        .complete(&structured())
        .await
        .unwrap_err();
    (failure.kind, failure.reason_code)
}

/// Kanata's admission codes are admission refusals only with their own
/// status, with a seconds `Retry-After`; `upstream_unavailable` opens the
/// backend only with the gateway's cooldown; timeouts, rate limits and
/// generic statuses keep their kinds.
#[tokio::test]
async fn gateway_replies_are_classified_by_status_code_and_retry_after() {
    use ProviderFailureKind::*;
    let refused = |secs: Option<u64>| AdmissionRefused {
        retry_after: secs.map(Duration::from_secs),
    };
    let down = |secs| BackendUnavailable {
        retry_after: Some(Duration::from_secs(secs)),
    };
    let limited = RateLimited {
        retry_after: Duration::from_secs(5),
    };
    let (queue_full, busy) = (Some("gateway_queue_full"), Some("gateway_busy"));
    let unavailable = Some("upstream_unavailable");
    let (admission, server) = ("admission", "server-error");
    let cases = [
        (429, queue_full, Some("3"), refused(Some(3)), admission),
        (
            429,
            Some("gateway_key_busy"),
            Some("3"),
            refused(Some(3)),
            admission,
        ),
        (
            429,
            Some("gateway_key_rate_limited"),
            Some("3"),
            refused(Some(3)),
            admission,
        ),
        (503, busy, Some("3"), refused(Some(3)), admission),
        (429, queue_full, None, refused(None), admission),
        // An HTTP-date Retry-After is ignored; a huge one is capped.
        (
            503,
            busy,
            Some("Wed, 21 Oct 2026 07:28:00 GMT"),
            refused(None),
            admission,
        ),
        (
            503,
            busy,
            Some("999999999999"),
            refused(Some(3_600)),
            admission,
        ),
        // Draining is a short requeue, not a backend failure.
        (
            503,
            Some("server_draining"),
            None,
            refused(None),
            "draining",
        ),
        // Admission codes count only with their own status.
        (503, queue_full, None, Transient, server),
        (503, Some("gateway_key_busy"), None, Transient, server),
        (
            503,
            Some("gateway_key_rate_limited"),
            None,
            Transient,
            server,
        ),
        (429, busy, None, Transient, "rate-limited"),
        (
            429,
            Some("server_draining"),
            None,
            Transient,
            "rate-limited",
        ),
        (400, queue_full, Some("5"), Permanent, "status"),
        (502, unavailable, None, Transient, server),
        (
            503,
            unavailable,
            Some("30"),
            down(30),
            "backend-unavailable",
        ),
        (503, unavailable, Some("5"), down(5), "backend-unavailable"),
        // Adapter down without Kanata's breaker counts toward our threshold.
        (503, unavailable, None, Transient, "backend-unavailable"),
        (
            504,
            Some("upstream_timeout"),
            Some("5"),
            UpstreamTimeout,
            "upstream-timeout",
        ),
        (504, None, Some("5"), UpstreamTimeout, "upstream-timeout"),
        (
            429,
            Some("rate_limit_exceeded"),
            Some("5"),
            limited,
            "rate-limited",
        ),
        (429, None, Some("5"), limited, "rate-limited"),
        (503, None, Some("5"), Transient, server),
    ];
    for (status, code, retry_after, kind, reason_code) in cases {
        let case = format!("{status} {code:?} {retry_after:?}");
        assert_eq!(
            classify(status, code, retry_after).await,
            (kind, reason_code),
            "{case}"
        );
    }
}

#[tokio::test]
async fn an_expired_key_is_reported_distinctly() {
    let stub = Stub::start(models_or(|_| {
        Reply::Json(
            401,
            json!({"error": {"message": "API key has expired", "type": "authentication_error", "code": "key_expired"}}),
        )
    }))
    .await;
    let error = runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::KeyExpired);
    let stub = Stub::start(models_or(|_| Reply::Json(401, json!({})))).await;
    let error = runner(declared(stub.url()))
        .complete(&structured())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::ProviderAuthentication);
}

#[tokio::test]
async fn the_runner_never_retries_admission_refusals_or_a_down_backend() {
    for (status, code, expected) in [
        (429, "gateway_queue_full", ErrorCode::AdmissionRefused),
        (503, "upstream_unavailable", ErrorCode::BackendUnavailable),
    ] {
        let stub = Stub::start(models_or(move |_| {
            Reply::Raw(raw(status, Some(code), Some("1")))
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

pub(super) fn client(
    provider: Arc<OpenAiCompatibleProvider>,
) -> ModelClient<OpenAiCompatibleProvider> {
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
