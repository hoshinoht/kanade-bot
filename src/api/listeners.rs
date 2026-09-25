//! Per-listener policy and router assembly. Authorization is by mounting:
//! the public router is built from `public::routes` and never sees an admin route.

use std::{net::IpAddr, path::PathBuf, sync::Arc};

use axum::{Router, extract::DefaultBodyLimit, middleware::from_fn_with_state, routing::get};

use super::{admin, assets, auth::AdminAuth, error, guard, public};
use crate::runtime::config::HttpConfig;

/// Offline mode has no Discord bot user to name the masthead after.
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
    pub identity_name: String,
    pub limits: guard::limits::Limits,
    /// Admin sign-in; `None` answers `auth_unavailable`. Never set on the public site.
    pub auth: Option<Arc<AdminAuth>>,
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
            limits: guard::limits::Limits::default(),
            auth: None,
        }
    }
}

pub fn router(mut site: Site) -> Router {
    if site.origin == Origin::Public {
        // Admin credentials must mean nothing on the public origin.
        site.auth = None;
    }
    let site = Arc::new(site);
    let routes = match site.origin {
        Origin::Admin => admin::routes(),
        Origin::Public => public::routes(),
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
