//! Failure classification for gateway admission, backend-down and timeout
//! replies. The Kanata codes are provisional (not shipped yet); loopback stubs only.

use std::time::Duration;

use kanade::infrastructure::llm::{ErrorCode, LlmProvider, ProviderFailureKind};
use serde_json::json;

use super::{
    http_transport::{declared, models_or, runner, structured},
    stub::{Reply, Stub},
};

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
            ProviderFailureKind::Transient,
        ),
        (429, None, ProviderFailureKind::Transient),
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
