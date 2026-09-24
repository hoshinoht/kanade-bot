use http_body_util::{BodyExt, Empty, Limited};
use hyper::{
    Request, StatusCode,
    body::{Body, Bytes},
    client::conn::http1,
};
use hyper_util::rt::TokioIo;
use tokio::{net::TcpStream, time::timeout};

use crate::runtime::{config::HealthcheckConfig, error::Error};

const MAX_RESPONSE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_HEADERS: usize = 32;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HealthResponse {
    status: String,
    mode: String,
    scheduler: String,
    storage: String,
    discord: String,
}

impl HealthResponse {
    fn is_expected_offline_response(&self) -> bool {
        self.status == "ok"
            && self.mode == "offline"
            && self.scheduler == "unavailable"
            && self.storage == "unavailable"
            && self.discord == "unavailable"
    }
}

pub async fn check(config: HealthcheckConfig) -> Result<(), Error> {
    timeout(config.timeout, check_inner(config.target))
        .await
        .map_err(|_| Error::Unavailable("healthcheck timed out".into()))?
}

async fn check_inner(target: std::net::SocketAddr) -> Result<(), Error> {
    let stream = TcpStream::connect(target)
        .await
        .map_err(|_| Error::Unavailable("healthcheck target is unavailable".into()))?;
    let mut builder = http1::Builder::new();
    builder
        .max_headers(MAX_HEADERS)
        .max_buf_size(MAX_HEADER_BYTES);
    let (mut sender, connection) = builder.handshake(TokioIo::new(stream)).await.map_err(|_| {
        Error::Unavailable("healthcheck target returned an invalid HTTP response".into())
    })?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = Request::get("/healthz")
        .header("host", "localhost")
        .body(Empty::<Bytes>::new())
        .map_err(|_| Error::Unavailable("healthcheck request could not be constructed".into()))?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|_| Error::Unavailable("healthcheck target is unavailable".into()))?;
    if response.status() != StatusCode::OK {
        return Err(Error::Unavailable(
            "healthcheck target returned a non-200 response".into(),
        ));
    }
    if response
        .body()
        .size_hint()
        .upper()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(Error::Unavailable(
            "healthcheck response exceeded size limit".into(),
        ));
    }
    let body = Limited::new(response.into_body(), MAX_RESPONSE_BYTES)
        .collect()
        .await
        .map_err(|_| Error::Unavailable("healthcheck response exceeded size limit".into()))?
        .to_bytes();
    let health: HealthResponse = serde_json::from_slice(&body).map_err(|_| {
        Error::Unavailable("healthcheck target returned invalid health JSON".into())
    })?;
    if !health.is_expected_offline_response() {
        return Err(Error::Unavailable(
            "healthcheck target is not offline-ready".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::{io::AsyncWriteExt, net::TcpListener};

    use super::*;

    async fn fake_response(response: Vec<u8>, hold_open: bool) -> HealthcheckConfig {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(&response).await.unwrap();
            if hold_open {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
        HealthcheckConfig {
            target,
            timeout: Duration::from_millis(100),
        }
    }

    #[tokio::test]
    async fn accepts_only_the_complete_expected_health_document() {
        let body = br#"{"status":"ok","mode":"offline","scheduler":"unavailable","storage":"unavailable","discord":"unavailable"}"#;
        let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        let config = fake_response([response.as_bytes(), body].concat(), false).await;
        assert!(check(config).await.is_ok());
    }

    #[tokio::test]
    async fn rejects_non_json_headers_and_non_success_statuses_without_echoing_response_content() {
        for response in [
            b"HTTP/1.1 200 OK\r\nX-Marker: leaked-secret\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/healthz\r\nContent-Length: 0\r\n\r\n".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nnot-json!".as_slice(),
        ] {
            let error = check(fake_response(response.to_vec(), false).await)
                .await
                .unwrap_err();
            assert!(!error.to_string().contains("leaked-secret"));
        }
    }

    #[tokio::test]
    async fn rejects_known_and_chunked_oversized_bodies_and_stalled_streams() {
        let known = b"HTTP/1.1 200 OK\r\nContent-Length: 8193\r\n\r\n";
        assert_eq!(
            check(fake_response(known.to_vec(), false).await)
                .await
                .unwrap_err()
                .to_string(),
            "healthcheck response exceeded size limit"
        );
        let chunked = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2001\r\n{}\r\n0\r\n\r\n",
            "x".repeat(MAX_RESPONSE_BYTES + 1)
        );
        assert_eq!(
            check(fake_response(chunked.into_bytes(), false).await)
                .await
                .unwrap_err()
                .to_string(),
            "healthcheck response exceeded size limit"
        );
        let stalled = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n";
        assert_eq!(
            check(fake_response(stalled.to_vec(), true).await)
                .await
                .unwrap_err()
                .to_string(),
            "healthcheck timed out"
        );

        let oversized_header = format!(
            "HTTP/1.1 200 OK\r\nX-Padding: {}\r\n\r\n",
            "x".repeat(MAX_HEADER_BYTES)
        );
        assert_eq!(
            check(fake_response(oversized_header.into_bytes(), false).await)
                .await
                .unwrap_err()
                .to_string(),
            "healthcheck target is unavailable"
        );
    }
}
