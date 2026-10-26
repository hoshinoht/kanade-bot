//! A server without the bot (the `web` component alone) that has the bot
//! token: the guild read over Discord's REST API at startup and every
//! [`EVERY`], so channel pickers, the access report and the roster stay
//! current (a member who left or lost the bossing role has their admin and
//! portal sessions ended, as the bot would). It never posts or reads
//! messages; guides still cannot publish ([`crate::app::ports::OfflineGuides`]).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::runtime::serve::bot::discord::{GatewayTransport, ports::StoreRoster, scope};
use crate::{
    api::state::ApplicationPort,
    bot::{
        events::{GuildRoles, GuildScope},
        gateway::ConnectionStatus,
        roster::{LiveRoster, RosterTask},
    },
    discord::{guild_cache::GuildCache, transport::Outcome},
    infrastructure::store::SqliteStore,
    runtime::{
        config::ServeConfig,
        error::Error,
        serve::{bot::tick, budget::ShutdownClock, web::api::Composition},
    },
};

/// How often the guild is read again.
pub const EVERY: Duration = Duration::from_secs(5 * 60);
/// How long the stop lets a pass in flight finish before cutting it.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// The running reader.
pub struct RestGuild {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RestGuild {
    /// Attach Setup's application read and the preview's difficulty marks
    /// (both read through `transport`, as the bot does), then start reading
    /// the guild into `cache`. Call before HTTP serves: it attaches to the
    /// API state.
    pub async fn start<T: GatewayTransport>(
        config: &ServeConfig,
        store: Arc<SqliteStore>,
        composition: &mut Composition,
        cache: Arc<GuildCache>,
        transport: Arc<T>,
        every: Duration,
    ) -> Result<Self, Error> {
        let marks = tick::difficulty_marks(&*transport).await;
        let application: ApplicationPort = {
            let transport = Arc::clone(&transport);
            Arc::new(move || {
                let transport = Arc::clone(&transport);
                Box::pin(async move {
                    match transport.application_info().await {
                        Outcome::Delivered(info) => Some(info),
                        _ => None,
                    }
                })
            })
        };
        match Arc::get_mut(&mut composition.admin.state) {
            Some(state) => {
                state.marks = marks;
                state.application = Some(application);
            }
            None => {
                return Err(Error::Startup(
                    "the guild reader could not be attached".into(),
                ));
            }
        }
        cache.set_watch(tick::watch_list(&composition.settings));
        let scope = scope(config);
        let (stop, stopped) = watch::channel(false);
        let roster = RosterTask::new(
            StoreRoster {
                store,
                hooks: composition.admin.roster_hooks(),
                access: Arc::clone(&composition.access),
            },
            Arc::clone(&transport),
            Arc::clone(&cache),
            scope,
            Arc::new(LiveRoster::new(Arc::clone(&cache))),
            // No gateway: only the one-shot reconcile is used, never its
            // generation gating.
            ConnectionStatus::new(),
            stopped.clone(),
        );
        let reader = Reader {
            transport,
            cache,
            scope,
            roster,
            ready: false,
        };
        let task = tokio::spawn(reader.run(stopped, every));
        Ok(Self { stop, task })
    }

    /// Ends the loop; a pass in flight gets [`STOP_GRACE`] (less if the
    /// shutdown budget is nearer), then is cut.
    pub async fn stop(self, clock: &ShutdownClock) {
        let _ = self.stop.send(true);
        let mut task = self.task;
        if clock
            .bounded("rest_guild", STOP_GRACE, &mut task)
            .await
            .is_none()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

struct Reader<T: GatewayTransport> {
    transport: Arc<T>,
    cache: Arc<GuildCache>,
    scope: GuildScope,
    roster: RosterTask<StoreRoster, T>,
    /// The transport knows the application (it is built from it).
    ready: bool,
}

impl<T: GatewayTransport> Reader<T> {
    async fn run(mut self, mut stopped: watch::Receiver<bool>, every: Duration) {
        loop {
            if self.pass(&stopped).await {
                kanade_log::log!(info, "rest_guild_read");
            }
            tokio::select! {
                () = tokio::time::sleep(every) => {}
                _ = stopped.changed() => return,
            }
        }
    }

    /// The bot's user, the guild (owner, roles) with its channels, then the
    /// roster. `false` when a read failed; nothing after it is concluded.
    async fn pass(&mut self, stopped: &watch::Receiver<bool>) -> bool {
        if !self.ready {
            match self.transport.application_info().await {
                Outcome::Delivered(info) => {
                    self.transport.application_ready(info.id);
                    self.ready = true;
                }
                other => return failed("application", &other),
            }
        }
        let me = match self.transport.current_user().await {
            Outcome::Delivered(user) => user,
            other => return failed("current_user", &other),
        };
        self.cache.set_self(me.id);
        let mut guild = match self.transport.guild(self.scope.guild_id).await {
            Outcome::Delivered(guild) => guild,
            other => return failed("guild", &other),
        };
        guild.channels = match self.transport.guild_channels(self.scope.guild_id).await {
            Outcome::Delivered(channels) => channels,
            other => return failed("channels", &other),
        };
        if *stopped.borrow() {
            return false;
        }
        self.cache.reset(&guild);
        let mut roles = GuildRoles::new(self.scope.guild_id);
        roles.reset(guild.owner_id, &guild.roles);
        self.roster
            .reconcile_now(guild.owner_id, &roles.admin_roles())
            .await
    }
}

fn failed<T>(step: &'static str, outcome: &Outcome<T>) -> bool {
    kanade_log::log!(
        warn,
        "rest_guild_read_failed",
        step,
        outcome = outcome.failure_label(),
    );
    false
}
