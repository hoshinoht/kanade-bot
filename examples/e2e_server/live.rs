//! The served world: one config, and generations of the real composition
//! (`compose_for_tests`, serve's wiring without the gateway) each over its
//! own store file. A generation is a copy of the template, so a reset is a
//! restart, and a file per generation means a request still holding the
//! old store never sees the new one change under it.

use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    http::{
        StatusCode,
        header::{CONTENT_TYPE, COOKIE, HOST, LOCATION, SET_COOKIE, USER_AGENT},
    },
    routing::{MethodRouter, any_service},
};
use http_body_util::Full;
use hyper::{Request, body::Bytes};
use hyper_util::rt::TokioIo;
use kanade::{
    api::{
        assets,
        auth::{
            self, Clock, crypto,
            fake::FakeDiscord,
            wire::{self, query_pairs, query_value},
        },
        listeners::{Site, router},
        server::LiveAdmin,
    },
    infrastructure::store::{SqliteStore, SqliteStoreConfig},
    runtime::{
        config::ServeConfig,
        serve::{
            CharacterDataModule,
            api::{Composition, Overrides, TestComposition, compose_for_tests},
        },
    },
};
use tokio::net::{TcpListener, TcpStream};

use crate::{
    Fail, LOOPBACK,
    bot::Bot,
    fakes::{discord, gateway, guild::Guild},
    seed,
};

/// As serve: how long closing waits for requests still holding the store.
const CLOSE_WAIT: Duration = Duration::from_secs(5);
/// How long the template's Discord side must make no model call before its
/// startup work (the day's header batch) counts as done.
const SETTLE: Duration = Duration::from_millis(1_500);
/// The browser the template's Discord admin signed in with: the e2e
/// browser's own kind, so its Account lists "Chrome · macOS" as this one.
const BROWSER: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

pub struct World {
    config: ServeConfig,
    /// Both listeners' policy before a generation fills in its live parts.
    admin: Site,
    public: Site,
    clock: Clock,
    discord: Arc<FakeDiscord>,
    authorize_url: String,
    nexon: SocketAddr,
    model: Arc<gateway::Activity>,
    /// The Limits page's canned governor shows three groups, not one
    /// (`/__e2e/limits`); every generation reads it live.
    pub three_groups: Arc<AtomicBool>,
}

/// The optional modules a generation composes with (serve reads them once
/// at startup, so a change is a restart: `/__e2e/modules`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modules {
    pub roster: bool,
    pub character_data: bool,
}

/// What each origin forwards to: the real listeners' routers.
pub struct Routes {
    pub admin: MethodRouter,
    pub public: MethodRouter,
}

pub struct Generation {
    path: PathBuf,
    store: Arc<SqliteStore>,
    composition: Composition,
    character_data: Option<CharacterDataModule>,
    bot: Option<Bot>,
}

impl World {
    pub fn new(
        config: ServeConfig,
        clock: Clock,
        discord: Arc<FakeDiscord>,
        authorize_url: String,
        nexon: SocketAddr,
        model: Arc<gateway::Activity>,
    ) -> Result<Self, Fail> {
        let http = &config.runtime.http;
        let mut admin = Site::admin(http);
        admin.listener_ip = Some(LOOPBACK);
        // The browser is the admin's trusted edge too (serve refuses a
        // loopback edge in config, so the config names another one): its
        // `Tailscale-*` headers count only with the edge secret, as behind
        // a real edge.
        admin.trusted_peer = Some(LOOPBACK);
        admin.edge_secret = http
            .edge_secret_file
            .as_deref()
            .map(auth::edge_secret)
            .transpose()?
            .map(Arc::new);
        let mut public = Site::public(http).ok_or("no public host")?;
        public.listener_ip = Some(LOOPBACK);
        public.public_origin = config
            .runtime
            .public_auth
            .discord
            .as_ref()
            .and_then(|discord| assets::origin_of(&discord.redirect_uri));
        Ok(Self {
            config,
            admin,
            public,
            clock,
            discord,
            authorize_url,
            nexon,
            model,
            three_groups: Arc::default(),
        })
    }

    /// The modules the config turns on.
    pub fn modules(&self) -> Modules {
        Modules {
            roster: self.config.modules.roster,
            character_data: self.config.modules.character_data,
        }
    }

    /// Compose over the store at `path` (and module `character_data`'s file
    /// beside it) with `modules`, as serve composes at startup, then start
    /// serve's Discord side when `bot`.
    pub async fn open(
        &self,
        path: PathBuf,
        bot: bool,
        modules: Modules,
    ) -> Result<Generation, Fail> {
        let store = SqliteStore::open(&SqliteStoreConfig {
            db_path: path.clone(),
            owner_lock_dir: self.config.store.owner_lock_dir.clone(),
        })
        .await?;
        let store = Arc::new(store);
        let mut config = self.config.clone();
        config.store.db_path = path.clone();
        config.modules.roster = modules.roster;
        config.modules.character_data = modules.character_data;
        if !modules.character_data {
            config.character_data = None;
        }
        if let Some(settings) = &mut config.character_data {
            settings.db_path = chardata(&path);
        }
        let overrides = Overrides {
            clock: Arc::clone(&self.clock),
            discord: Some(self.discord.clone()),
            authorize_url: Some(self.authorize_url.clone()),
            // The governor the Limits page shows, canned (`fakes::gateway`).
            model_limits: Some({
                let three = Arc::clone(&self.three_groups);
                Arc::new(move |now| gateway::limits(now, three.load(Ordering::SeqCst)))
            }),
            nexon: Some(self.nexon),
        };
        match compose_for_tests(&config, store.clone(), overrides).await {
            Ok(TestComposition {
                mut composition,
                character_data,
            }) => {
                // The gateway's guild cache as the seeded guild shows it.
                let installed = Arc::get_mut(&mut composition.admin.state)
                    .map(|state| state.channels = Arc::new(Guild))
                    .is_some();
                let generation = Generation {
                    path,
                    store,
                    composition,
                    character_data,
                    bot: None,
                };
                if !installed {
                    let _ = generation.close().await;
                    return Err("the composed state is already shared".into());
                }
                if !bot {
                    return Ok(generation);
                }
                generation.with_bot(config, Arc::clone(&self.clock)).await
            }
            Err(error) => {
                if let Err(close) = close_store(store).await {
                    eprintln!("e2e_server: {close}");
                }
                Err(error.into())
            }
        }
    }

    /// Compose over a fresh store at `path`, seed it, sign the admins in and
    /// keep it closed as the template. Returns the admin sessions every copy
    /// holds.
    pub async fn template(&self, path: &Path, token: &str) -> Result<AdminSessions, Fail> {
        let generation = self.open(path.to_owned(), false, self.modules()).await?;
        let live = generation.live();
        let seeded = match seed::template(generation.store.clone(), &live.state).await {
            Ok(()) => match seed::auth_extra::seed(&generation.store, (live.state.clock)()).await {
                Ok(()) => self.sign_in_admins(live, token).await,
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        };
        generation.close().await?;
        let admins = seeded?;
        // Serve's Discord side once over the seeded store, so what it does
        // at startup (journal recovery, the day's header batch) is in every
        // copy and a generation's own start finds nothing left to do.
        let generation = self.open(path.to_owned(), true, self.modules()).await?;
        self.model.settled(SETTLE).await;
        generation.close().await?;
        // A WAL left beside either file would be missing from every copy.
        if wal(path).exists() || wal(&chardata(path)).exists() {
            return Err("the template store left a WAL behind".into());
        }
        Ok(admins)
    }

    /// Copy the template's files to a generation at `to`.
    pub fn copy_template(&self, from: &Path, to: &Path) -> io::Result<()> {
        std::fs::copy(from, to)?;
        if self.config.character_data.is_some() {
            std::fs::copy(chardata(from), chardata(to))?;
        }
        Ok(())
    }

    pub fn routes(&self, live: &LiveAdmin) -> Routes {
        Routes {
            admin: any_service(router(self.admin_site(live))),
            public: any_service(router(self.public_site(live))),
        }
    }

    /// The core primitives showcase (`bun run showcase`), as the admin
    /// listener serves its app: its own files under the production headers.
    pub fn showcase(&self) -> Router {
        let mut site = self.admin.clone();
        site.app_dir = site.app_dir.map(|dist| dist.join("__showcase"));
        router(site)
    }

    fn admin_site(&self, live: &LiveAdmin) -> Site {
        let mut site = self.admin.clone();
        site.auth = Some(Arc::clone(&live.auth));
        site.state = Some(Arc::clone(&live.state));
        site.health = Some(Arc::clone(&live.health));
        site
    }

    fn public_site(&self, live: &LiveAdmin) -> Site {
        let mut site = self.public.clone();
        site.bot = Some(Arc::clone(&live.state.channels));
        site.member = live.member.clone();
        site.state = Some(Arc::clone(&live.state));
        site
    }

    /// The real sign-ins, through the admin router on a throwaway loopback
    /// listener: the break-glass token login, then Asahi's Discord login
    /// (the start, the fake Discord's approval, the callback).
    async fn sign_in_admins(&self, live: &LiveAdmin, token: &str) -> Result<AdminSessions, Fail> {
        let host = self
            .config
            .runtime
            .http
            .admin_host
            .clone()
            .ok_or("no admin host")?;
        let listener = TcpListener::bind((LOOPBACK, 0)).await?;
        let address = listener.local_addr()?;
        let app = router(self.admin_site(live));
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });
        let signed_in = async {
            let body = serde_json::to_vec(&serde_json::json!({ "token": token }))?;
            let request = Request::post("/api/admin/auth/token")
                .header(HOST, &host)
                .header("sec-fetch-site", "same-origin")
                .header(CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(body)))?;
            let response = send(address, request).await?;
            if response.status() != StatusCode::OK {
                return Err(format!("token sign-in answered {}", response.status()).into());
            }
            let token_session = set_cookies(&response)
                .into_iter()
                .next()
                .ok_or("token sign-in set no cookie")?;

            let request = Request::get("/api/admin/auth/discord/start")
                .header(HOST, &host)
                .header(USER_AGENT, BROWSER)
                .body(Full::default())?;
            let start = send(address, request).await?;
            let location = start
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| format!("Discord sign-in start answered {}", start.status()))?;
            let pairs = query_pairs(location.split_once('?').map(|(_, query)| query));
            let (Some(state), Some(challenge)) = (
                query_value(&pairs, "state"),
                query_value(&pairs, "code_challenge"),
            ) else {
                return Err(format!("Discord sign-in start sent {location}").into());
            };
            let code = crypto::random_token().ok_or("no system randomness")?;
            self.discord
                .approve(&code, &challenge, discord::user(seed::ids::ASAHI));
            let cookies = set_cookies(&start)
                .into_iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join("; ");
            let request = Request::get(format!(
                "/api/admin/auth/discord/callback?{}",
                wire::form(&[("code", &code), ("state", &state)])
            ))
            .header(HOST, &host)
            .header(USER_AGENT, BROWSER)
            .header(COOKIE, cookies)
            .body(Full::default())?;
            let callback = send(address, request).await?;
            let discord_session = set_cookies(&callback)
                .into_iter()
                .find(|(name, value)| *name == token_session.0 && !value.is_empty())
                .ok_or_else(|| {
                    format!(
                        "Discord sign-in answered {} without a session ({:?})",
                        callback.status(),
                        callback.headers().get(LOCATION)
                    )
                })?;
            Ok::<_, Fail>(AdminSessions {
                discord: discord_session,
                token: token_session,
            })
        }
        .await;
        server.abort();
        let _ = server.await;
        signed_in
    }
}

/// A cookie: name, value.
pub type Cookie = (String, String);

/// The admin sessions every template copy holds.
pub struct AdminSessions {
    /// Asahi's Discord session: the admin every spec browses as, as on the
    /// mock (approving Kanade's proposals needs a Discord admin).
    pub discord: Cookie,
    /// The break-glass token session.
    pub token: Cookie,
}

/// One request on a fresh connection to `address`.
async fn send(
    address: SocketAddr,
    request: Request<Full<Bytes>>,
) -> Result<hyper::Response<hyper::body::Incoming>, Fail> {
    let tcp = TcpStream::connect(address).await?;
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(tcp)).await?;
    tokio::spawn(connection);
    Ok(sender.send_request(request).await?)
}

/// The `name=value` of each cookie a response sets.
fn set_cookies<B>(response: &hyper::Response<B>) -> Vec<Cookie> {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
        .collect()
}

impl Generation {
    pub fn live(&self) -> &LiveAdmin {
        &self.composition.admin
    }

    pub fn store(&self) -> &Arc<SqliteStore> {
        &self.store
    }

    /// Starts serve's Discord side over this generation's composition.
    async fn with_bot(self, config: ServeConfig, clock: Clock) -> Result<Self, Fail> {
        let Self {
            path,
            store,
            composition,
            character_data,
            bot: _,
        } = self;
        match Bot::start(config, Arc::clone(&store), composition, clock).await {
            Ok((composition, bot)) => Ok(Self {
                path,
                store,
                composition,
                character_data,
                bot: Some(bot),
            }),
            Err(error) => {
                if let Some(module) = character_data {
                    module.stop(CLOSE_WAIT).await;
                }
                if let Err(close) = close_store(store).await {
                    eprintln!("e2e_server: {close}");
                }
                Err(error)
            }
        }
    }

    /// Stops its Discord side, ends its event streams, stops what else it
    /// runs and closes its store.
    pub async fn close(self) -> Result<PathBuf, Fail> {
        let Self {
            path,
            store,
            composition,
            character_data,
            bot,
        } = self;
        if let Some(bot) = bot {
            bot.stop().await;
        }
        // Open streams hold the store until they end.
        composition.admin.state.events.close();
        drop(composition);
        if let Some(module) = character_data {
            module.stop(CLOSE_WAIT).await;
        }
        close_store(store).await?;
        Ok(path)
    }

    /// [`Self::close`], then delete its files.
    pub async fn retire(self) {
        match self.close().await {
            Ok(path) => {
                let chardata = chardata(&path);
                for file in [&path, &chardata]
                    .into_iter()
                    .flat_map(|file| [file.clone(), wal(file), sibling(file, "-shm")])
                {
                    match std::fs::remove_file(&file) {
                        Err(error) if error.kind() != io::ErrorKind::NotFound => {
                            eprintln!("e2e_server: could not remove {}: {error}", file.display());
                        }
                        _ => {}
                    }
                }
            }
            Err(error) => eprintln!("e2e_server: a retired store did not close: {error}"),
        }
    }
}

/// Waits up to [`CLOSE_WAIT`] for the last other holder, as serve's close does.
async fn close_store(mut store: Arc<SqliteStore>) -> Result<(), Fail> {
    let deadline = Instant::now() + CLOSE_WAIT;
    loop {
        match Arc::try_unwrap(store) {
            Ok(owned) => return Ok(owned.close().await?),
            Err(shared) if Instant::now() < deadline => {
                store = shared;
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            Err(_) => return Err("the store is still in use".into()),
        }
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn wal(path: &Path) -> PathBuf {
    sibling(path, "-wal")
}

/// Module `character_data`'s file for the store at `path`.
fn chardata(path: &Path) -> PathBuf {
    path.with_extension("chardata.sqlite3")
}
