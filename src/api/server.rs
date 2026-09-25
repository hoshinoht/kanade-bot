use std::{io, net::SocketAddr, sync::Arc};

use axum::Router;
use tokio::{net::TcpListener, sync::watch, time::timeout};

use super::{
    auth::{self, AdminAuth},
    listeners::{self, Site},
    state::ApiState,
};
use crate::runtime::{application::HealthProbe, config::RuntimeConfig, error::Error, logging};

/// What a live admin listener serves beyond the offline shell.
pub struct LiveAdmin {
    pub auth: Arc<AdminAuth>,
    pub state: Arc<ApiState>,
    pub health: Arc<dyn HealthProbe>,
}

pub async fn serve_offline(config: RuntimeConfig) -> Result<(), Error> {
    serve(&config, None, wait_for_shutdown()).await
}

/// Both listeners until `shutdown`, then a drain bounded by the configured
/// deadline. Everything `live` holds is dropped by the time this returns
/// (unless the deadline abandoned connections still hold it).
pub async fn serve(
    config: &RuntimeConfig,
    live: Option<LiveAdmin>,
    shutdown: impl Future<Output = ()>,
) -> Result<(), Error> {
    let mode = if live.is_some() { "live" } else { "offline" };
    let mut admin_site = Site::admin(&config.http);
    if let Some(path) = &config.http.edge_secret_file {
        admin_site.edge_secret = Some(Arc::new(auth::edge_secret(path)?));
    }
    admin_site.listener_ip = Some(config.admin_bind.ip());
    if let Some(live) = live {
        admin_site.auth = Some(live.auth);
        admin_site.state = Some(live.state);
        admin_site.health = Some(live.health);
    }
    let admin = bind(config.admin_bind, mode).await?;
    let public = match (config.public_bind, Site::public(&config.http)) {
        (Some(address), Some(mut site)) => {
            site.listener_ip = Some(address.ip());
            Some((bind(address, mode).await?, site))
        }
        (Some(_), None) => {
            return Err(Error::Configuration(
                "KANADE_PUBLIC_HOST is required when KANADE_PUBLIC_BIND is set".into(),
            ));
        }
        (None, _) => None,
    };

    let (stop, stopped) = watch::channel(());
    let admin_server = run(admin, listeners::router(admin_site), stopped.clone());
    let public_server = async move {
        match public {
            Some((listener, site)) => run(listener, listeners::router(site), stopped).await,
            None => Ok(()),
        }
    };
    let servers = async { tokio::try_join!(admin_server, public_server).map(|_| ()) };
    tokio::pin!(servers);

    tokio::select! {
        result = &mut servers => result.map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into())),
        () = shutdown => {
            logging::shutdown_started();
            let _ = stop.send(());
            timeout(config.shutdown_timeout, &mut servers)
                .await
                .map_err(|_| Error::Startup("graceful shutdown exceeded configured deadline".into()))?
                .map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into()))
        }
    }
}

async fn bind(address: SocketAddr, mode: &'static str) -> Result<TcpListener, Error> {
    let listener = TcpListener::bind(address)
        .await
        .map_err(|_| Error::Startup("unable to bind configured address".into()))?;
    logging::server_started(
        mode,
        &listener
            .local_addr()
            .map_err(|_| Error::Startup("unable to inspect bound address".into()))?
            .to_string(),
    );
    Ok(listener)
}

/// Peer addresses feed the proxy guard; `stopped` begins a graceful drain.
async fn run(
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

/// `SIGINT` or `SIGTERM`.
pub async fn wait_for_shutdown() {
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
