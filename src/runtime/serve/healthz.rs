//! The admin listener of a bot-only serve: `GET /healthz` for clients on
//! this host (the local healthcheck), `404` for everything else. Plain hyper,
//! so it needs neither the admin nor the public router.

use std::{
    convert::Infallible,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    header::{CACHE_CONTROL, CONTENT_TYPE, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinSet,
    time::timeout,
};

use super::budget::ShutdownClock;
use crate::runtime::{application::HealthProbe, config::RuntimeConfig, error::Error, logging};

/// After a failed accept (out of descriptors), before the next one.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Serves until `shutdown` yields the started budget, then drains open
/// connections as the admin router does: cut by the budget is logged and
/// `Ok`, cut by the configured deadline an error.
pub async fn serve(
    config: &RuntimeConfig,
    health: Arc<dyn HealthProbe>,
    shutdown: impl Future<Output = ShutdownClock>,
) -> Result<(), Error> {
    let listener = TcpListener::bind(config.admin_bind)
        .await
        .map_err(|_| Error::Startup("unable to bind configured address".into()))?;
    let local = listener
        .local_addr()
        .map_err(|_| Error::Startup("unable to inspect bound address".into()))?;
    logging::server_started("live", &local.to_string());
    let (stop, stopped) = watch::channel(());
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    let clock = loop {
        tokio::select! {
            clock = &mut shutdown => break clock,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    connections.spawn(connection(
                        stream,
                        peer,
                        local.ip(),
                        Arc::clone(&health),
                        stopped.clone(),
                    ));
                }
                Err(_) => tokio::time::sleep(ACCEPT_BACKOFF).await,
            },
            // Reaped as they end, so the set holds only open connections.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    };
    drop(listener);
    logging::shutdown_started();
    let _ = stop.send(());
    let drain = clock.phase(config.shutdown_timeout);
    let drained = timeout(drain, async {
        while connections.join_next().await.is_some() {}
    })
    .await;
    match drained {
        Ok(()) => Ok(()),
        Err(_) if drain < config.shutdown_timeout => {
            clock.cut("http_drain");
            Ok(())
        }
        Err(_) => Err(Error::Startup(
            "graceful shutdown exceeded configured deadline".into(),
        )),
    }
}

/// One HTTP/1 connection; `stopped` lets its request in flight finish, then
/// closes it.
async fn connection(
    stream: TcpStream,
    peer: SocketAddr,
    listener_ip: IpAddr,
    health: Arc<dyn HealthProbe>,
    mut stopped: watch::Receiver<()>,
) {
    let service = service_fn(move |request: Request<Incoming>| {
        let health = Arc::clone(&health);
        async move { Ok::<_, Infallible>(answer(&request, peer, listener_ip, &*health).await) }
    });
    let connection = http1::Builder::new().serve_connection(TokioIo::new(stream), service);
    tokio::pin!(connection);
    tokio::select! {
        _ = connection.as_mut() => {}
        _ = stopped.changed() => {
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

/// The admin router's `/healthz` rule: only a client on this host
/// (loopback or the listener's own address) gets the document.
async fn answer(
    request: &Request<Incoming>,
    peer: SocketAddr,
    listener_ip: IpAddr,
    health: &dyn HealthProbe,
) -> Response<Full<Bytes>> {
    let local = peer.ip().is_loopback() || peer.ip() == listener_ip;
    let method = request.method();
    if !local
        || request.uri().path() != "/healthz"
        || !(method == Method::GET || method == Method::HEAD)
    {
        return respond(StatusCode::NOT_FOUND, Bytes::new());
    }
    let health = health.health().await;
    let status = if health.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = serde_json::to_vec(&health).expect("the health document serializes");
    let mut response = respond(status, Bytes::from(body));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn respond(status: StatusCode, body: Bytes) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(body));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
