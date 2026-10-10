//! Per-listener policy and router assembly. Authorization is by mounting:
//! the public router is built from `public::routes` and never sees an admin route.

use std::{net::IpAddr, path::PathBuf, sync::Arc};

use axum::{Router, extract::DefaultBodyLimit, middleware::from_fn_with_state, routing::get};

use super::{
    admin, assets,
    auth::{AdminAuth, crypto::SealedSecret, member::MemberAuth},
    error, guard, public,
    state::{ApiState, ChannelList},
};
use crate::runtime::{application::HealthProbe, config::HttpConfig};

/// The masthead name before `READY`, and in offline mode.
const OFFLINE_IDENTITY_NAME: &str = "Kanade";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Admin,
    Public,
}

impl Origin {
    fn app(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Public => "public",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPolicy {
    /// Exact lowercase `host[:port]`.
    Exact(String),
    /// `localhost`, `127.0.0.1` or `[::1]` on any port (unconfigured admin).
    LoopbackNames,
}

#[derive(Clone, Debug)]
pub struct Site {
    pub origin: Origin,
    pub hosts: HostPolicy,
    /// The only peer whose forwarding/identity headers survive the proxy guard.
    pub trusted_peer: Option<IpAddr>,
    /// Built app directory (`index.html` + assets).
    pub app_dir: Option<PathBuf>,
    pub boss_dir: Option<PathBuf>,
    pub identity_dir: Option<PathBuf>,
    /// Shown until the gateway names the bot.
    pub identity_name: String,
    /// The live bot name for the public origin, which has no `state`.
    pub bot: Option<Arc<dyn ChannelList>>,
    /// Public only: `https://host` of the validated member redirect URI, for
    /// the shell's absolute link-preview URLs; `None` leaves them out.
    pub public_origin: Option<String>,
    pub limits: guard::limits::Limits,
    /// Admin sign-in; `None` answers `auth_unavailable`. Never set on the public site.
    pub auth: Option<Arc<AdminAuth>>,
    /// Member sign-in; `None` keeps the portal closed. Never set on the admin site.
    pub member: Option<Arc<MemberAuth>>,
    /// Admin only: the edge must present this in `X-Kanade-Edge-Auth` to be trusted.
    pub edge_secret: Option<Arc<SealedSecret>>,
    /// The listener's bound address: a client there is on this host (healthcheck).
    pub listener_ip: Option<IpAddr>,
    /// The read state; `None` answers `unavailable`. On the public site only
    /// the member reads behind the session use it.
    pub state: Option<Arc<ApiState>>,
    /// Live `/healthz`; `None` reports offline mode.
    pub health: Option<Arc<dyn HealthProbe>>,
}

impl Site {
    pub fn admin(http: &HttpConfig) -> Self {
        Self::new(
            Origin::Admin,
            http.admin_host
                .clone()
                .map_or(HostPolicy::LoopbackNames, HostPolicy::Exact),
            http.trusted_proxy,
            http,
        )
    }

    /// `None` without a public host: that listener must not exist.
    pub fn public(http: &HttpConfig) -> Option<Self> {
        let host = http.public_host.clone()?;
        Some(Self::new(
            Origin::Public,
            HostPolicy::Exact(host),
            http.cloudflared_peer,
            http,
        ))
    }

    fn new(
        origin: Origin,
        hosts: HostPolicy,
        trusted_peer: Option<IpAddr>,
        http: &HttpConfig,
    ) -> Self {
        Self {
            origin,
            hosts,
            trusted_peer,
            app_dir: http
                .web_dir
                .as_ref()
                .map(|web| web.join("apps").join(origin.app()).join("dist")),
            boss_dir: http.boss_dir.clone(),
            identity_dir: http.identity_dir.clone(),
            identity_name: OFFLINE_IDENTITY_NAME.into(),
            bot: None,
            public_origin: None,
            limits: guard::limits::Limits::default(),
            auth: None,
            member: None,
            edge_secret: None,
            listener_ip: None,
            state: None,
            health: None,
        }
    }
}

pub fn router(mut site: Site) -> Router {
    match site.origin {
        // Admin credentials must mean nothing on the public origin.
        Origin::Public => {
            site.auth = None;
            site.edge_secret = None;
            site.health = None;
        }
        // Nor a member session on the admin origin.
        Origin::Admin => site.member = None,
    }
    let site = Arc::new(site);
    let routes = match site.origin {
        Origin::Admin => admin::routes(),
        Origin::Public => public::routes(site.clone()),
    };
    // Last layer runs first: headers wrap every answer, including guard refusals.
    routes
        .route("/api/identity", get(assets::identity))
        .route("/identity/avatar", get(assets::avatar))
        .route("/identity/banner", get(assets::banner))
        .fallback(assets::fallback)
        .method_not_allowed_fallback(error::method_not_allowed)
        .with_state(site.clone())
        .layer(DefaultBodyLimit::max(site.limits.body_bytes))
        .layer(from_fn_with_state(site.clone(), guard::limits::enforce))
        .layer(from_fn_with_state(site.clone(), guard::proxy::sanitize))
        .layer(from_fn_with_state(site.clone(), guard::host::enforce))
        .layer(from_fn_with_state(site, guard::headers::apply))
}
