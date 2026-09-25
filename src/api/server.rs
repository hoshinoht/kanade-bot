use std::{io, net::SocketAddr, sync::Arc};

use axum::Router;
use tokio::{net::TcpListener, sync::watch, time::timeout};

use super::{
    auth,
    listeners::{self, Site},
};
use crate::runtime::{config::RuntimeConfig, error::Error, logging};

pub async fn serve_offline(config: RuntimeConfig) -> Result<(), Error> {
    let mut admin_site = Site::admin(&config.http);
    if let Some(path) = &config.http.edge_secret_file {
        admin_site.edge_secret = Some(Arc::new(auth::edge_secret(path)?));
    }
    admin_site.listener_ip = Some(config.admin_bind.ip());
    let admin = bind(config.admin_bind).await?;
    let public = match (config.public_bind, Site::public(&config.http)) {
        (Some(address), Some(mut site)) => {
            site.listener_ip = Some(address.ip());
            Some((bind(address).await?, site))
        }
        (Some(_), None) => {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_HOST is required when KANADE_PUBLIC_BIND is set".into(),
            ));
        }
        (None, _) => None,
    };

    let (stop, stopped) = watch::channel(());
    let admin_server = serve(admin, listeners::router(admin_site), stopped.clone());
    let public_server = async move {
        match public {
            Some((listener, site)) => serve(listener, listeners::router(site), stopped).await,
            None => Ok(()),
        }
    };
    let servers = async { tokio::try_join!(admin_server, public_server).map(|_| ()) };
    tokio::pin!(servers);

    tokio::select! {
        result = &mut servers => result.map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into())),
        () = wait_for_shutdown() => {
            logging::shutdown_started();
            let _ = stop.send(());
            timeout(config.shutdown_timeout, &mut servers)
                .await
                .map_err(|_| Error::Startup("graceful shutdown exceeded configured deadline".into()))?
                .map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into()))
        }
    }
}

async fn bind(address: SocketAddr) -> Result<TcpListener, Error> {
    let listener = TcpListener::bind(address)
        .await
        .map_err(|_| Error::Startup("unable to bind configured address".into()))?;
    logging::server_started(
        &listener
            .local_addr()
            .map_err(|_| Error::Startup("unable to inspect bound address".into()))?
            .to_string(),
    );
    Ok(listener)
}

/// Peer addresses feed the proxy guard; `stopped` begins a graceful drain.
async fn serve(
    listener: TcpListener,
    router: Router,
    mut stopped: watch::Receiver<()>,
) -> io::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = stopped.changed().await;
    })
    .await
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
