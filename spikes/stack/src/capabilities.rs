use axum::{Router, routing::get};
use chrono::{TimeZone, Utc};
use chrono_tz::America::Toronto;
use tokio::net::TcpListener;
use tower_http::services::ServeFile;

pub fn timezone_probe() -> String {
    Toronto
        .from_utc_datetime(
            &Utc.with_ymd_and_hms(2026, 11, 1, 6, 30, 0)
                .unwrap()
                .naive_utc(),
        )
        .to_rfc3339()
}

pub async fn http_asset_health_probe() -> Result<(), Box<dyn std::error::Error>> {
    let app = app();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        let mut stream = tokio::net::TcpStream::connect(address).await?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        Ok::<_, std::io::Error>(response)
    })
    .await??;
    server.abort();
    if !response.contains("200 OK") || !response.ends_with("ok") {
        return Err("health response mismatch".into());
    }
    Ok(())
}

pub async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    axum::serve(listener, app())
        .with_graceful_shutdown(async {
            let ctrl_c = async {
                tokio::signal::ctrl_c()
                    .await
                    .expect("install Ctrl-C handler")
            };
            #[cfg(unix)]
            let terminate = async {
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("install SIGTERM handler")
                    .recv()
                    .await;
            };
            #[cfg(not(unix))]
            let terminate = std::future::pending::<()>();
            tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
        })
        .await?;
    Ok(())
}

fn app() -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route_service("/asset.txt", ServeFile::new("fixtures/asset.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use rustls::{
        ClientConfig, RootCertStore, ServerConfig,
        pki_types::{PrivateKeyDer, ServerName},
    };
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};
    use tokio_tungstenite::{accept_async, connect_async, tungstenite::Message};

    #[test]
    fn timezone_uses_iana_dst_rules() {
        assert_eq!(timezone_probe(), "2026-11-01T01:30:00-05:00");
    }

    #[tokio::test]
    async fn health_and_static_asset_are_served() {
        http_asset_health_probe().await.unwrap();
    }

    #[tokio::test]
    async fn rustls_accepts_trusted_cert_and_rejects_untrusted_cert() {
        // Provider comes from the shared application seam, not test-local setup.
        crate::tls::install_default_provider();
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let certificate = certified.cert.der().clone();
        let key = PrivateKeyDer::Pkcs8(certified.signing_key.serialize_der().into());
        let server = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![certificate.clone()], key)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, _) = listener.accept().await.unwrap();
                let acceptor = TlsAcceptor::from(Arc::new(server.clone()));
                if let Ok(mut stream) = acceptor.accept(stream).await {
                    let mut message = [0; 3];
                    stream.read_exact(&mut message).await.unwrap();
                    assert_eq!(&message, b"tls");
                    stream.write_all(b"ok").await.unwrap();
                }
            }
        });
        let wrong = TlsConnector::from(Arc::new(
            ClientConfig::builder()
                .with_root_certificates(RootCertStore::empty())
                .with_no_client_auth(),
        ));
        assert!(
            wrong
                .connect(
                    ServerName::try_from("localhost").unwrap(),
                    tokio::net::TcpStream::connect(address).await.unwrap()
                )
                .await
                .is_err()
        );
        let mut roots = RootCertStore::empty();
        roots.add(certificate).unwrap();
        let trusted = TlsConnector::from(Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ));
        let mut stream = trusted
            .connect(
                ServerName::try_from("localhost").unwrap(),
                tokio::net::TcpStream::connect(address).await.unwrap(),
            )
            .await
            .unwrap();
        stream.write_all(b"tls").await.unwrap();
        let mut response = [0; 2];
        stream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"ok");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn websocket_upgrade_and_echo_are_local() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let message = socket.next().await.unwrap().unwrap();
            socket.send(message).await.unwrap();
        });
        let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
        socket
            .send(Message::Text("synthetic".into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text("synthetic".into())
        );
        server.await.unwrap();
    }
}
