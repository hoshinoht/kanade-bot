use std::{io, net::SocketAddr, sync::Arc};

use axum::Router;
use tokio::{net::TcpListener, sync::watch, time::timeout};

use super::{
    assets,
    auth::{self, AdminAuth, member::MemberAuth},
    listeners::{self, Site},
    state::ApiState,
};
use crate::runtime::{
    application::HealthProbe, config::RuntimeConfig, error::Error, logging,
    serve::budget::ShutdownClock,
};

/// What a live server serves beyond the offline shell.
pub struct LiveAdmin {
    pub auth: Arc<AdminAuth>,
    pub state: Arc<ApiState>,
    pub health: Arc<dyn HealthProbe>,
    /// Member sign-in for the public listener; `None` keeps the portal closed.
    pub member: Option<Arc<MemberAuth>>,
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
    let unbounded = async {
        shutdown.await;
        ShutdownClock::default()
    };
    serve_bounded(config, live, unbounded).await
}

/// [`serve`] whose drain also ends where the started shutdown budget
/// `shutdown` yields leaves off (before the store reserve). A drain cut by
/// the budget is logged and returns `Ok`; one cut by the configured
/// deadline stays an error.
pub async fn serve_bounded(
    config: &RuntimeConfig,
    live: Option<LiveAdmin>,
    shutdown: impl Future<Output = ShutdownClock>,
) -> Result<(), Error> {
    let mode = if live.is_some() { "live" } else { "offline" };
    let mut admin_site = Site::admin(&config.http);
    if let Some(path) = &config.http.edge_secret_file {
        admin_site.edge_secret = Some(Arc::new(auth::edge_secret(path)?));
    }
    admin_site.listener_ip = Some(config.admin_bind.ip());
    let bot = live.as_ref().map(|live| Arc::clone(&live.state.channels));
    let events = live.as_ref().map(|live| Arc::clone(&live.state.events));
    // The member reads behind the public session read the same state.
    let reads = live.as_ref().map(|live| Arc::clone(&live.state));
    let mut member = None;
    if let Some(live) = live {
        admin_site.auth = Some(live.auth);
        admin_site.state = Some(live.state);
        admin_site.health = Some(live.health);
        member = live.member;
    }
    let admin = bind(config.admin_bind, mode).await?;
    let public = match (config.public_bind, Site::public(&config.http)) {
        (Some(address), Some(mut site)) => {
            site.listener_ip = Some(address.ip());
            site.bot = bot;
            site.member = member;
            site.state = reads;
            site.public_origin = config
                .public_auth
                .discord
                .as_ref()
                .and_then(|discord| assets::origin_of(&discord.redirect_uri));
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
        clock = shutdown => {
            logging::shutdown_started();
            // Open event streams never finish on their own: end them first so
            // the drain is not held to the deadline.
            if let Some(events) = &events {
                events.close();
            }
            let _ = stop.send(());
            let drain = clock.phase(config.shutdown_timeout);
            match timeout(drain, &mut servers).await {
                Ok(result) => result.map_err(|_| Error::Startup("HTTP server stopped unexpectedly".into())),
                Err(_) if drain < config.shutdown_timeout => {
                    clock.cut("http_drain");
                    Ok(())
                }
                Err(_) => Err(Error::Startup("graceful shutdown exceeded configured deadline".into())),
            }
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
