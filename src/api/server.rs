use std::{convert::Infallible, future::IntoFuture};

use axum::{Json, Router, routing::get};
use tokio::{net::TcpListener, sync::oneshot, time::timeout};

use crate::runtime::{
    application::OfflineApplication, config::RuntimeConfig, error::Error, logging,
};

pub async fn serve_offline(config: RuntimeConfig) -> Result<(), Error> {
    let listener = TcpListener::bind(config.bind)
        .await
        .map_err(|_| Error::Startup("unable to bind configured address".into()))?;
    logging::server_started(
        &listener
            .local_addr()
            .map_err(|_| Error::Startup("unable to inspect bound address".into()))?
            .to_string(),
    );

    let (shutdown_started, mut shutdown_observer) = oneshot::channel();
    let graceful_shutdown = async move {
        wait_for_shutdown().await;
        logging::shutdown_started();
        let _ = shutdown_started.send(());
    };
    let server = axum::serve(listener, routes())
        .with_graceful_shutdown(graceful_shutdown)
        .into_future();
    tokio::pin!(server);

    tokio::select! {
        result = &mut server => result.map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into())),
        _ = &mut shutdown_observer => timeout(config.shutdown_timeout, &mut server)
            .await
            .map_err(|_| Error::Startup("graceful shutdown exceeded configured deadline".into()))?
            .map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into())),
    }
}

fn routes() -> Router {
    Router::new().route("/healthz", get(health))
}

async fn health() -> Result<Json<crate::runtime::application::Health>, Infallible> {
    Ok(Json(OfflineApplication.health()))
}

async fn wait_for_shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { () = ctrl_c => {}, () = terminate => {} }
}
