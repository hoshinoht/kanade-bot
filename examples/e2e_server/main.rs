//! The browser e2e server (`web/playwright.config.ts`) and the local dev
//! server (`bun run serve:dev` in `web/`): the real `kanade` admin and
//! public routers, composed as `serve` composes them (`compose_for_tests`,
//! then serve's Discord side over a fake transport and a gateway that never
//! connects: [`bot`]) over a temporary SQLite store, with the harness routes
//! of [`controls`] in front.
//!
//! `--admin-port A --public-port P --web-dir PATH [--now RFC3339] [--boss-dir
//! PATH]` serves `127.0.0.1:A` and `127.0.0.1:P` until SIGINT/SIGTERM. With
//! `--now` (e2e: `2026-09-29T04:00:00Z`, Tue 12:00 in the guild's Kuala
//! Lumpur) the clock stands still there until `/__e2e/clock` moves it;
//! without it the clock is live (the seed is relative to the current boss
//! week). Boss art comes from `--boss-dir` (default the placeholder
//! `web/e2e/fixtures/boss`; `KANADE_REAL_ART=1` reviews point it at the
//! private repo-root `boss/`). It reads no environment; everything it writes
//! lives under a 0700 directory in the temp dir, removed on exit.

mod bot;
mod controls;
mod controls_guides;
mod controls_p6;
mod fakes;
mod guides;
mod live;
mod seed;

use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs::{self, DirBuilder, OpenOptions},
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use kanade::{
    api::{
        auth::{crypto, fake::FakeDiscord},
        listeners::Origin,
        server::wait_for_shutdown,
    },
    runtime::config::ServeConfig,
};
use tokio::{net::TcpListener, sync::watch};

use controls::{Shared, SharedClock, Start};

pub type Fail = Box<dyn Error + Send + Sync>;
pub const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
/// How long the listeners may drain on shutdown.
const DRAIN: Duration = Duration::from_secs(5);
/// The admin edge's shared secret (`X-Kanade-Edge-Auth`): whoami hands it
/// to the specs, which act as the edge for a Tailscale sign-in.
pub const EDGE_SECRET: &str = "e2e-edge-secret-shared-with-the-browser-tests";
/// The one Tailscale login the admin allows (the mock's Tailscale admin).
pub const TAILSCALE_LOGIN: &str = "ops@example.test";

struct Args {
    admin_port: u16,
    public_port: u16,
    web_dir: PathBuf,
    /// `None`: the live clock.
    now: Option<DateTime<Utc>>,
    boss_dir: Option<PathBuf>,
}

fn parse(mut values: impl Iterator<Item = String>) -> Result<Args, Fail> {
    let (mut admin_port, mut public_port, mut web_dir, mut now, mut boss_dir) =
        (None, None, None, None, None);
    while let Some(flag) = values.next() {
        let value = values.next().ok_or_else(|| format!("missing {flag}"))?;
        let slot = match flag.as_str() {
            "--admin-port" => &mut admin_port,
            "--public-port" => &mut public_port,
            "--web-dir" => &mut web_dir,
            "--now" => &mut now,
            "--boss-dir" => &mut boss_dir,
            _ => return Err(format!("unknown argument {flag}").into()),
        };
        if slot.replace(value).is_some() {
            return Err(format!("duplicate argument {flag}").into());
        }
    }
    let port = |value: Option<String>, flag: &str| -> Result<u16, Fail> {
        let port = value.ok_or_else(|| format!("missing {flag}"))?.parse()?;
        if port == 0 {
            return Err(format!("{flag} must be a port").into());
        }
        Ok(port)
    };
    let admin_port = port(admin_port, "--admin-port")?;
    let public_port = port(public_port, "--public-port")?;
    if admin_port == public_port {
        return Err("--admin-port and --public-port must differ".into());
    }
    let now = now
        .map(|now| {
            DateTime::parse_from_rfc3339(&now)
                .map(|now| now.with_timezone(&Utc))
                .map_err(|_| "--now must be an RFC 3339 instant")
        })
        .transpose()?;
    Ok(Args {
        admin_port,
        public_port,
        web_dir: fs::canonicalize(web_dir.ok_or("missing --web-dir")?)?,
        now,
        boss_dir: boss_dir.map(fs::canonicalize).transpose()?,
    })
}

/// The temporary root, under the canonical temp dir (the store refuses
/// symlinked paths such as macOS's `/var`); removed on drop.
struct Root(PathBuf);

impl Root {
    fn create() -> Result<Self, Fail> {
        let base = fs::canonicalize(env::temp_dir())?;
        let root = Self(base.join(format!("kanade-e2e-{}", uuid::Uuid::new_v4().simple())));
        private_dir(&root.0)?;
        for dir in [
            "db",
            "backups",
            "locks",
            "Personas",
            "Personas/bundles",
            "Personas/profiles",
        ] {
            private_dir(&root.0.join(dir))?;
        }
        Ok(root)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn private_dir(path: &Path) -> std::io::Result<()> {
    DirBuilder::new().mode(0o700).create(path)
}

fn secret(path: &Path, value: &str) -> std::io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(value.as_bytes())
}

/// Serve's config as a deployment would give it, pointed at the root, the
/// fakes and the tracked catalog, knowledge, persona and fixture art.
fn config(
    args: &Args,
    root: &Path,
    fakes: &fakes::Fakes,
    token: &str,
) -> Result<ServeConfig, Fail> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    let path = |relative: &str| root.join(relative).display().to_string();
    // The root is the secrets directory: each secret under its fixed name.
    secret(
        &root.join("discord_token"),
        "e2e-discord-token-not-a-real-secret",
    )?;
    secret(&root.join("admin_token"), token)?;
    secret(
        &root.join("discord_client_secret"),
        "e2e-fake-client-secret",
    )?;
    secret(
        &root.join("public_discord_client_secret"),
        "e2e-fake-public-client-secret",
    )?;
    secret(&root.join("nexon_api_key"), "e2e-fake-nexon-key")?;
    secret(&root.join("edge_secret"), EDGE_SECRET)?;
    seed::persona_files(
        &root.join("Personas"),
        &repo.join("config/personas/bundles/kanade.yaml"),
    )?;
    let admin = format!("127.0.0.1:{}", args.admin_port);
    let public = format!("127.0.0.1:{}", args.public_port);
    let values: BTreeMap<String, String> = [
        ("KANADE_TIMEZONE", "Asia/Kuala_Lumpur".to_owned()),
        ("KANADE_ADMIN_BIND", admin.clone()),
        ("KANADE_ADMIN_HOST", admin.clone()),
        ("KANADE_PUBLIC_BIND", public.clone()),
        ("KANADE_PUBLIC_HOST", public.clone()),
        // The browser is the edge's loopback peer.
        ("KANADE_CLOUDFLARED_PEER", LOOPBACK.to_string()),
        ("KANADE_DISCORD_GATEWAY", "0".to_owned()),
        ("KANADE_SECRETS_DIR", root.display().to_string()),
        // Setup saves into it; a reset empties it (`World::forget_saved_secrets`).
        (
            "KANADE_SAVED_SECRETS_DIR",
            root.join("saved").display().to_string(),
        ),
        // Tailscale sign-in behind an edge. Serve refuses a loopback edge, so
        // the config names a private one and `live::World` trusts the
        // browser's loopback peer instead, still only with the edge secret.
        ("KANADE_TRUSTED_PROXY", "10.0.0.2".to_owned()),
        ("KANADE_ADMIN_TAILSCALE_LOGINS", TAILSCALE_LOGIN.to_owned()),
        // The member realm's longest idle limit, as the mock's portal keeps
        // Asahi's 40-minutes-idle iPhone session live.
        ("KANADE_PUBLIC_SESSION_IDLE_MINUTES", "60".to_owned()),
        // The template's admin session must outlast a full suite run; both
        // stay within serve's caps.
        ("KANADE_ADMIN_SESSION_IDLE_MINUTES", "720".to_owned()),
        ("KANADE_ADMIN_SESSION_ABSOLUTE_HOURS", "24".to_owned()),
        // Sign-in redirects follow the hosts: http for the loopback admin,
        // https for the public origin (the fake page maps it back).
        ("KANADE_ADMIN_DISCORD_CLIENT_ID", "424242".to_owned()),
        ("KANADE_PUBLIC_DISCORD_CLIENT_ID", "424243".to_owned()),
        ("KANADE_GUILD_ID", "900".to_owned()),
        // The mock's guild roles (`seed::ROLES`): staff, bossers, pilots.
        ("KANADE_BOSSING_ROLE_ID", seed::BOSSING_ROLE.to_owned()),
        ("KANADE_ADMIN_ROLE_ID", seed::STAFF_ROLE.to_owned()),
        ("KANADE_CHAT_PILOT_ROLE_ID", seed::PILOT_ROLE.to_owned()),
        ("KANADE_CHAT_CATEGORY_IDS", "410000000000000001".to_owned()),
        // The seeded guild's watched channels (`seed::CHANNELS`).
        ("KANADE_WATCH_CHANNEL_IDS", seed::watched_ids()),
        // The mock's digest channel (Config › Channel access marks it).
        (
            "KANADE_POST_CHANNEL_ID",
            seed::ids::BOSS_SCHEDULE.to_owned(),
        ),
        ("KANADE_MODULE_ROSTER", "1".to_owned()),
        ("KANADE_MODULE_CHARACTER_DATA", "1".to_owned()),
        // The guide publisher over the fake Discord (`guides`).
        ("KANADE_MODULE_GUIDES", "1".to_owned()),
        (
            "KANADE_CHARACTER_DATA_DB_PATH",
            path("db/template.chardata.sqlite3"),
        ),
        ("KANADE_DB_PATH", path("db/template.sqlite3")),
        // History → Checkpoints lists the manifests `seed::backups` writes.
        ("KANADE_BACKUP_DIR", path("backups")),
        ("KANADE_OWNER_LOCK_DIR", path("locks")),
        ("KANADE_WEB_DIR", args.web_dir.display().to_string()),
        (
            "KANADE_BOSS_DIR",
            args.boss_dir
                .clone()
                .unwrap_or_else(|| args.web_dir.join("e2e/fixtures/boss"))
                .display()
                .to_string(),
        ),
        (
            "KANADE_CATALOG_FILE",
            repo.join("boss/bosses.yaml").display().to_string(),
        ),
        (
            "KANADE_KNOWLEDGE_DIR",
            repo.join("boss/knowledge").display().to_string(),
        ),
        ("KANADE_PERSONA_DIR", path("Personas")),
        (
            "KANADE_MODEL_BASE_URL",
            format!("http://{}/v1", fakes.gateway),
        ),
        // The mock's role aliases (published by the fake gateway's catalog),
        // their declarations (used only once `/__e2e/gateway` drops the
        // Kanata metadata) and its one permit.
        ("KANADE_EXTRACT_MODEL", "kanata/extract".to_owned()),
        ("KANADE_CHAT_MODEL", "kanata/chat".to_owned()),
        ("KANADE_REWRITE_MODEL", "kanata/rewrite-small".to_owned()),
        ("KANADE_MODEL_CAPABILITIES", fakes::gateway::declarations()),
        ("KANADE_MODEL_PERMITS", "1".to_owned()),
        ("KANADE_SCOUTER_URL", format!("http://{}", fakes.scouter)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    Ok(ServeConfig::from_mapping(&values)?)
}

#[tokio::main]
async fn main() -> Result<(), Fail> {
    let started = Instant::now();
    // Sign-in builds its HTTPS client before the fake Discord replaces it.
    kanade::runtime::tls::install_ring_provider()?;
    let args = parse(env::args().skip(1))?;
    let root = Root::create()?;
    let fakes = fakes::start().await?;
    let token = crypto::random_token().ok_or("no system randomness")?;
    let config = config(&args, &root.0, &fakes, &token)?;
    let admin = format!("http://127.0.0.1:{}", args.admin_port);
    let public = format!("http://127.0.0.1:{}", args.public_port);
    let discord = Arc::new(FakeDiscord::default());
    let page = Arc::new(fakes::discord::Page {
        discord: discord.clone(),
        redirects: vec![
            (
                format!("{admin}/api/admin/auth/discord/callback"),
                format!("{admin}/api/admin/auth/discord/callback"),
            ),
            (
                format!(
                    "https://127.0.0.1:{}/api/public/auth/discord/callback",
                    args.public_port
                ),
                format!("{public}/api/public/auth/discord/callback"),
            ),
        ],
    });
    let clock = args.now.map_or_else(SharedClock::live, SharedClock::pinned);
    let world = live::World::new(
        config,
        clock.clock(),
        discord,
        format!("{admin}{}", fakes::discord::PATH),
        fakes.nexon,
        Arc::clone(&fakes.model),
        Arc::clone(&fakes.chardata),
    )?;
    let db_dir = root.0.join("db");
    let template = db_dir.join("template.sqlite3");
    let admins = world.template(&template, &token).await?;
    let shared = Arc::new(
        Shared::start(Start {
            world,
            db_dir,
            template,
            clock,
            admins,
            token,
            page,
        })
        .await?,
    );

    let admin_listener = TcpListener::bind((LOOPBACK, args.admin_port)).await?;
    let public_listener = TcpListener::bind((LOOPBACK, args.public_port)).await?;
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "e2e_server_ready",
            "admin": admin,
            "public": public,
            "startup_ms": started.elapsed().as_millis(),
        })
    );
    let (stop, stopped) = watch::channel(());
    let serve = |listener: TcpListener, origin| {
        let mut stopped = stopped.clone();
        axum::serve(
            listener,
            controls::router(&shared, origin).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = stopped.changed().await;
        })
        .into_future()
    };
    let servers = async {
        tokio::try_join!(
            serve(admin_listener, Origin::Admin),
            serve(public_listener, Origin::Public),
        )
    };
    tokio::pin!(servers);
    let served = tokio::select! {
        served = &mut servers => served.map(|_| ()),
        () = wait_for_shutdown() => {
            shared.end_streams().await;
            let _ = stop.send(());
            let _ = tokio::time::timeout(DRAIN, &mut servers).await;
            Ok(())
        }
    };
    shared.close().await;
    drop(fakes);
    drop(root);
    Ok(served?)
}
