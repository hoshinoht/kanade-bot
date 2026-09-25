//! Dev-only PWA mock server: admin and public builds on two origins (two ports), each
//! with its own API surface, the production CSP, a CSP report sink, and
//! same-origin boss/identity art.

mod api;
mod assets;
#[cfg(test)]
mod contract;
mod headers;
mod mock;
mod reports;
mod writes;

use axum::{
    Router,
    extract::Request,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use mock::{Store, catalog::Catalog};
use std::{convert::Infallible, env, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tower::Service;
use tower_http::services::{ServeDir, ServeFile};

#[derive(Clone)]
struct App {
    store: Arc<Mutex<Store>>,
    reports: reports::Log,
    identity: assets::IdentityConfig,
    knowledge: Arc<mock::knowledge::KnowledgeDir>,
    /// Art answers `closed` on the public origin while the portal is closed.
    public: bool,
    boss_dir: Arc<PathBuf>,
    /// CSRF token and Idempotency-Key replays, shared by both origins' state.
    writes: Arc<writes::Writes>,
}

/// SPA fallback for extensionless paths only, so a missing asset is a 404 rather than HTML.
fn static_site(
    dist: PathBuf,
) -> ServeDir<impl Service<Request, Response = Response, Error = Infallible, Future: Send> + Clone>
{
    let index = ServeFile::new(dist.join("index.html"));
    let spa = tower::service_fn(move |req: Request| {
        let mut index = index.clone();
        async move {
            if req
                .uri()
                .path()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .contains('.')
            {
                return Ok::<_, Infallible>(StatusCode::NOT_FOUND.into_response());
            }
            index.call(req).await.map(IntoResponse::into_response)
        }
    });
    ServeDir::new(dist).fallback(spa)
}

fn common(app: &App, api: Router<App>, dist: PathBuf) -> Router {
    Router::new()
        .merge(api)
        .route("/api/identity", get(assets::identity))
        .route("/api/{*rest}", get(api::not_found).post(api::not_found))
        .route("/art/{kind}/{key}", get(assets::art))
        .route("/identity/avatar", get(assets::avatar))
        .route("/identity/banner", get(assets::banner))
        .route("/csp-report", post(reports::receive))
        .route("/__mock/reports", get(reports::list).delete(reports::clear))
        .route("/__mock/whoami", get(whoami))
        .route("/__mock/csrf/rotate", post(writes::rotate))
        .with_state(app.clone())
        .fallback_service(static_site(dist))
        .layer(middleware::from_fn(headers::apply))
}

/// Lets the e2e fixture prove it reached a mock it may drive: the right
/// binary, clock pinned, and which art it serves. Dev servers answer too,
/// with `now: null`, so a stray one fails the suite loudly.
async fn whoami(
    axum::extract::State(app): axum::extract::State<App>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "mock": "kanade-pwa-mock",
        "now": mock::clock::pinned_raw(),
        "boss_dir": app.boss_dir.display().to_string(),
        "origin": if app.public { "public" } else { "admin" },
    }))
}

fn path_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let web = path_env("KANADE_WEB_DIR").unwrap_or_else(|| "../../web".into());
    // Deployment-private art (git-ignored); tests point this at synthetic fixtures.
    let boss_dir = path_env("KANADE_BOSS_DIR").unwrap_or_else(|| web.join("../boss"));
    let admin_port = env::var("ADMIN_PORT").unwrap_or_else(|_| "4173".into());
    let public_port = env::var("PUBLIC_PORT").unwrap_or_else(|_| "4174".into());

    let app = App {
        store: Arc::new(Mutex::new(Store::new(Catalog::new(boss_dir.clone())))),
        reports: reports::Log::default(),
        // Tracked, public boss knowledge (schema v2).
        knowledge: Arc::new(mock::knowledge::KnowledgeDir(
            path_env("KANADE_KNOWLEDGE_DIR").unwrap_or_else(|| web.join("../boss/knowledge")),
        )),
        identity: assets::IdentityConfig {
            name: env::var("KANADE_BOT_NAME").unwrap_or_else(|_| "YuukiSakuna".into()),
            dir: path_env("KANADE_IDENTITY_DIR"),
        },
        public: false,
        boss_dir: Arc::new(boss_dir.clone()),
        writes: Arc::default(),
    };
    let (admin, public) = routers(app, &web);

    let admin_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{admin_port}")).await?;
    let public_listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{public_port}")).await?;
    eprintln!(
        "admin  http://127.0.0.1:{admin_port}\npublic http://127.0.0.1:{public_port}\nboss art {}",
        boss_dir.display()
    );

    tokio::try_join!(
        axum::serve(admin_listener, admin).with_graceful_shutdown(shutdown()),
        axum::serve(public_listener, public).with_graceful_shutdown(shutdown()),
    )?;
    Ok(())
}

/// The admin and public origins over one shared store.
fn routers(app: App, web: &std::path::Path) -> (Router, Router) {
    let public_app = App {
        public: true,
        ..app.clone()
    };

    // Authorization boundary by origin: the public origin has no admin routes at all.
    let admin_api = Router::new()
        .route("/api/admin/week", get(api::week))
        .route("/api/admin/stats", get(api::stats))
        .route("/api/admin/summary", get(api::summary))
        .route("/api/admin/members", get(api::members))
        .route("/api/admin/members/{id}", patch(api::patch_member))
        .route("/api/admin/members/{id}/aliases", post(api::add_alias))
        .route("/api/admin/personas", get(api::personas))
        .route("/api/admin/fixed", get(api::fixed).post(api::create_fixed))
        .route(
            "/api/admin/fixed/{id}",
            patch(api::update_fixed).delete(api::retire_fixed),
        )
        .route("/api/admin/validate/bosses", post(api::validate_bosses))
        .route("/api/admin/bosses", get(api::bosses))
        .route("/api/admin/bosses/{key}/knowledge", get(api::knowledge_v2))
        .route("/api/admin/bosses/events", get(api::events))
        .route("/api/admin/inbox", get(api::inbox))
        .route("/api/admin/inbox/{id}/approve", post(api::approve))
        .route("/api/admin/inbox/{id}/reject", post(api::reject))
        .route("/api/admin/extractions", get(api::extractions))
        .route("/api/admin/extractions/{id}", get(api::extraction))
        .route("/api/admin/rescan/targets", get(api::rescan_targets))
        .route("/api/admin/rescan", post(api::start_rescan))
        .route(
            "/api/admin/rescan/{id}",
            get(api::poll_rescan).delete(api::cancel_rescan),
        )
        .route("/api/admin/chat", get(api::chat))
        .route("/api/admin/chat/{id}", get(api::chat_turn))
        .route("/api/admin/limits", get(api::limits))
        .route("/api/admin/limits/windows/{id}", delete(api::reset_window))
        .route(
            "/api/admin/config",
            get(api::config).patch(api::patch_config),
        )
        .route("/api/admin/digest", post(api::digest))
        .route(
            "/api/admin/config/profiles/reload",
            post(api::reload_profiles),
        )
        .route("/api/admin/access", get(api::access))
        .route("/api/admin/access/recheck", post(api::access))
        .route("/api/admin/history", get(api::history))
        .route("/api/admin/history/checkpoints", get(api::checkpoints))
        .route("/api/admin/history/revert", post(api::revert))
        .route("/api/admin/history/restore-week", post(api::restore_week))
        .route("/api/admin/history/revert-actor", post(api::revert_actor))
        .route("/api/admin/history/{seq}", get(api::history_record))
        .route("/api/admin/runs/{id}/blame", get(api::blame))
        .route("/api/admin/reminders", get(api::reminders))
        .route("/api/admin/runs/{id}/reset", post(api::reset_run))
        .route("/api/admin/channels", get(api::channels))
        .route("/api/admin/session", get(api::session))
        .route("/api/admin/runs/{id}/move", post(api::move_run))
        .route("/api/admin/runs/{id}/status", patch(api::status))
        .route("/api/admin/runs/{id}/rsvp", post(api::rsvp))
        .route(
            "/api/admin/runs/{id}/participants",
            patch(api::participants),
        )
        .route("/api/admin/runs/{id}/ping", post(api::ping))
        .route("/api/admin/reset", post(api::reset))
        .route_layer(middleware::from_fn_with_state(app.clone(), writes::guard));
    let public_api = Router::new()
        .route("/api/public/week", get(api::public_week))
        .route("/api/public/status", get(api::public_status));

    (
        common(&app, admin_api, web.join("apps/admin/dist")),
        common(&public_app, public_api, web.join("apps/public/dist")),
    )
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}
