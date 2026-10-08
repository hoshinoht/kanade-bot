//! Rust-browser E2E fixtures.
//!
//! Default: seeds the isolated E2E store through the production scheduler
//! (`--db … --lock-dir … --timezone … --reset-weekday … --reset-time …`).
//!
//! `member-portal --public-port N --discord-port M --web-dir PATH [--closed]`:
//! serves the real public router with the member realm on an in-memory
//! store at `127.0.0.1:N` (Host `127.0.0.1:N`), plus a fake Discord
//! authorization page at `127.0.0.1:M`, until SIGINT/SIGTERM. Sign-in needs
//! no Discord: `/api/public/auth/discord/start` redirects to the fake page,
//! which approves at once and returns to the real callback. A spec picks who
//! signs in with the cookie `kanade_fake_discord` on `127.0.0.1` (cookies
//! ignore ports): `eligible` (default; user `100000000000000001`, "Mikan"),
//! `ineligible` (`100000000000000002`, no bossing role), `bot`
//! (`100000000000000003`, a bot account) or `deny` (cancelled at Discord).
//! `--closed` keeps the portal switch off.

use std::{
    env,
    error::Error,
    io,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, Duration, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;
use kanade::{
    domain::{
        history::{Actor, Origin, Surface},
        ids::RandomIds,
        schedule::{NewRun, RunSource, RunStatus, SchedulePolicy, utc_instant},
        scheduler::{Clock, SchedulerService},
        settings::RuntimeSettings,
    },
    infrastructure::store::{SqliteStore, SqliteStoreConfig},
};

#[derive(Debug)]
struct Args {
    db_path: PathBuf,
    owner_lock_dir: PathBuf,
    timezone: Tz,
    reset_weekday: Weekday,
    reset_time: NaiveTime,
}

#[derive(Clone, Copy)]
struct FixtureClock(DateTime<Utc>);

impl Clock for FixtureClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

fn required(value: Option<String>, name: &str) -> Result<String, io::Error> {
    value.ok_or_else(|| invalid(format!("missing {name}")))
}

fn parse<I>(values: I) -> Result<Args, io::Error>
where
    I: IntoIterator<Item = String>,
{
    let mut values = values.into_iter();
    let mut db_path = None;
    let mut owner_lock_dir = None;
    let mut timezone = None;
    let mut reset_weekday = None;
    let mut reset_time = None;

    while let Some(flag) = values.next() {
        let value = required(values.next(), &flag)?;
        let slot = match flag.as_str() {
            "--db" => &mut db_path,
            "--lock-dir" => &mut owner_lock_dir,
            "--timezone" => &mut timezone,
            "--reset-weekday" => &mut reset_weekday,
            "--reset-time" => &mut reset_time,
            _ => return Err(invalid(format!("unknown argument {flag}"))),
        };
        if slot.replace(value).is_some() {
            return Err(invalid(format!("duplicate argument {flag}")));
        }
    }

    let db_path = PathBuf::from(required(db_path, "--db")?);
    let owner_lock_dir = PathBuf::from(required(owner_lock_dir, "--lock-dir")?);
    if !db_path.is_absolute() || !owner_lock_dir.is_absolute() {
        return Err(invalid("--db and --lock-dir must be absolute"));
    }
    let timezone = required(timezone, "--timezone")?
        .parse()
        .map_err(|_| invalid("--timezone must be an IANA zone"))?;
    let reset_weekday = match required(reset_weekday, "--reset-weekday")?
        .to_ascii_lowercase()
        .as_str()
    {
        "mon" => Weekday::Mon,
        "tue" => Weekday::Tue,
        "wed" => Weekday::Wed,
        "thu" => Weekday::Thu,
        "fri" => Weekday::Fri,
        "sat" => Weekday::Sat,
        "sun" => Weekday::Sun,
        _ => return Err(invalid("--reset-weekday must be one of mon..sun")),
    };
    let reset_time = strict_time(&required(reset_time, "--reset-time")?)
        .ok_or_else(|| invalid("--reset-time must be HH:MM"))?;
    Ok(Args {
        db_path,
        owner_lock_dir,
        timezone,
        reset_weekday,
        reset_time,
    })
}

fn strict_time(value: &str) -> Option<NaiveTime> {
    let bytes = value.as_bytes();
    (bytes.len() == 5
        && bytes[2] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 2 || byte.is_ascii_digit()))
    .then(|| NaiveTime::from_hms_opt(value[0..2].parse().ok()?, value[3..5].parse().ok()?, 0))
    .flatten()
}

fn policy(args: &Args) -> SchedulePolicy {
    // This mirrors live serve's `settings::seed`: only the reset seed differs
    // from a fresh RuntimeSettings value, because the launcher sets only it.
    let mut settings = RuntimeSettings::default();
    settings.schedule.reset_weekday = args.reset_weekday;
    settings.schedule.reset_time = args.reset_time;
    settings.schedule_policy(args.timezone)
}

fn now() -> Result<DateTime<Utc>, io::Error> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("system clock precedes the Unix epoch"))?;
    DateTime::from_timestamp(elapsed.as_secs() as i64, elapsed.subsec_nanos())
        .ok_or_else(|| invalid("system clock is out of range"))
}

fn next_run(now: DateTime<Utc>, policy: &SchedulePolicy) -> Result<NewRun, Box<dyn Error>> {
    let this_week = utc_instant(&policy.week_of(&now)?)?;
    let week_start = this_week + Duration::days(7);
    // Day one at noon is always in the next boss week for the harness's reset
    // and stays ahead of the wall clock without a frozen-time assumption.
    let datetime = week_start + Duration::days(1) + Duration::hours(12);
    if utc_instant(&policy.week_of(&datetime)?)? != week_start || datetime <= now {
        return Err(invalid("could not choose a future next-week fixture slot").into());
    }
    Ok(NewRun {
        fixed_run_id: None,
        channel_id: None,
        week_start,
        datetime,
        bosses: vec!["HSeren".into()],
        participants: Vec::new(),
        status: RunStatus::Planned,
        source: RunSource::Amend,
    })
}

async fn seed(args: Args) -> Result<(), Box<dyn Error>> {
    let now = now()?;
    let policy = policy(&args);
    let store = SqliteStore::open(&SqliteStoreConfig {
        db_path: args.db_path,
        owner_lock_dir: args.owner_lock_dir,
    })
    .await?;
    let mut scheduler = SchedulerService::new(store, RandomIds, FixtureClock(now))
        .with_attendance(policy.attendance);
    let seeded = scheduler
        .as_origin(Origin::new(Actor::system("rust_e2e_fixture"), Surface::Cli))
        .create_run(next_run(now, &policy)?)
        .await;
    let closed = scheduler.into_store().close().await;
    seeded?;
    closed?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1).peekable();
    if args.peek().map(String::as_str) == Some("member-portal") {
        args.next();
        return member_portal::run(member_portal::parse(args)?).await;
    }
    seed(parse(args)?).await
}

mod member_portal {
    use std::{
        error::Error,
        net::SocketAddr,
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use axum::{
        Router,
        extract::State,
        http::{HeaderMap, StatusCode, Uri, header::COOKIE},
        response::{IntoResponse, Response},
        routing::get,
    };
    use kanade::{
        api::{
            auth::{
                crypto,
                discord::{DiscordClient, DiscordLogin, DiscordUser, Secret},
                fake::FakeDiscord,
                member::{MemberAuth, StoreEligibility},
                wire,
            },
            listeners::{Site, router},
        },
        domain::members::{GatewayMember, MemberStore},
        infrastructure::store::MemoryScheduleStore,
        runtime::config::HttpConfig,
    };
    use tokio::net::TcpListener;

    use super::{invalid, required};

    /// The cookie a spec sets on `127.0.0.1` to choose the fake Discord user.
    pub const USER_COOKIE: &str = "kanade_fake_discord";
    const ELIGIBLE: &str = "100000000000000001";
    const INELIGIBLE: &str = "100000000000000002";
    const BOT: &str = "100000000000000003";

    pub struct Args {
        public_port: u16,
        discord_port: u16,
        web_dir: PathBuf,
        open: bool,
    }

    pub fn parse(mut values: impl Iterator<Item = String>) -> Result<Args, Box<dyn Error>> {
        let (mut public_port, mut discord_port, mut web_dir, mut open) = (None, None, None, true);
        while let Some(flag) = values.next() {
            if flag == "--closed" {
                open = false;
                continue;
            }
            let value = required(values.next(), &flag)?;
            match flag.as_str() {
                "--public-port" => public_port = Some(value.parse::<u16>()?),
                "--discord-port" => discord_port = Some(value.parse::<u16>()?),
                "--web-dir" => web_dir = Some(PathBuf::from(value)),
                _ => return Err(invalid(format!("unknown argument {flag}")).into()),
            }
        }
        Ok(Args {
            public_port: public_port.ok_or_else(|| invalid("missing --public-port"))?,
            discord_port: discord_port.ok_or_else(|| invalid("missing --discord-port"))?,
            web_dir: web_dir.ok_or_else(|| invalid("missing --web-dir"))?,
            open,
        })
    }

    fn user(choice: &str) -> DiscordUser {
        let (id, name, bot) = match choice {
            "ineligible" => (INELIGIBLE, "Plain", false),
            "bot" => (BOT, "Botty", true),
            _ => (ELIGIBLE, "Mikan", false),
        };
        DiscordUser {
            id: id.into(),
            username: name.to_lowercase(),
            global_name: Some(name.into()),
            bot,
            avatar: None,
        }
    }

    struct FakePage {
        discord: Arc<FakeDiscord>,
        redirect: String,
    }

    /// Discord's authorization page, approving at once for the chosen user.
    async fn authorize(
        State(page): State<Arc<FakePage>>,
        headers: HeaderMap,
        uri: Uri,
    ) -> Response {
        let pairs = wire::query_pairs(uri.query());
        let value = |key| wire::query_value(&pairs, key);
        let (Some(state), Some(challenge), Some(redirect)) = (
            value("state"),
            value("code_challenge"),
            value("redirect_uri"),
        ) else {
            return StatusCode::BAD_REQUEST.into_response();
        };
        if redirect != page.redirect || value("scope").as_deref() != Some("identify") {
            return StatusCode::BAD_REQUEST.into_response();
        }
        let choice = headers
            .get_all(COOKIE)
            .iter()
            .filter_map(|header| header.to_str().ok())
            .flat_map(|header| header.split(';'))
            .filter_map(|pair| pair.trim().split_once('='))
            .find(|(name, _)| *name == USER_COOKIE)
            .map_or("eligible", |(_, value)| value);
        let back = if choice == "deny" {
            wire::form(&[("error", "access_denied"), ("state", &state)])
        } else {
            let Some(code) = crypto::random_token() else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            page.discord.approve(&code, &challenge, user(choice));
            wire::form(&[("code", &code), ("state", &state)])
        };
        wire::see_other(&format!("{redirect}?{back}"), [])
    }

    async fn seed_members(store: &MemoryScheduleStore) -> Result<(), Box<dyn Error>> {
        for (choice, has_role) in [("eligible", true), ("ineligible", false)] {
            let user = user(choice);
            store
                .apply_gateway(GatewayMember {
                    user_id: user.id.clone(),
                    display_name: Some(user.display()),
                    nickname: None,
                    has_role,
                    is_bot: false,
                    roles: Vec::new(),
                    is_guild_admin: false,
                })
                .await?;
        }
        Ok(())
    }

    async fn serve(address: SocketAddr, app: Router) -> Result<(), Box<dyn Error>> {
        let listener = TcpListener::bind(address).await?;
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(kanade::api::server::wait_for_shutdown())
        .await?;
        Ok(())
    }

    pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
        let host = format!("127.0.0.1:{}", args.public_port);
        let redirect = format!("http://{host}/api/public/auth/discord/callback");
        let store = Arc::new(MemoryScheduleStore::new());
        seed_members(&store).await?;
        let discord = Arc::new(FakeDiscord::default());
        let open = Arc::new(AtomicBool::new(args.open));
        let member = MemberAuth::new(
            store.clone(),
            Arc::new(StoreEligibility::new(store.clone())),
            Arc::new(move || open.load(Ordering::SeqCst)),
        )
        .with_discord(
            DiscordLogin::new(
                DiscordClient {
                    client_id: "424242".into(),
                    client_secret: Secret::new("e2e-fake-secret"),
                    redirect_uri: redirect.clone(),
                },
                discord.clone(),
            )
            .with_authorize_url(format!(
                "http://127.0.0.1:{}/oauth2/authorize",
                args.discord_port
            )),
        );
        let http = HttpConfig {
            public_host: Some(host),
            web_dir: Some(args.web_dir),
            ..HttpConfig::default()
        };
        let mut site = Site::public(&http).ok_or_else(|| invalid("no public host"))?;
        site.member = Some(Arc::new(member));
        let page = Arc::new(FakePage { discord, redirect });
        let fake = Router::new()
            .route("/oauth2/authorize", get(authorize))
            .with_state(page);
        tokio::try_join!(
            serve(([127, 0, 0, 1], args.public_port).into(), router(site)),
            serve(([127, 0, 0, 1], args.discord_port).into(), fake),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Args {
        parse(
            [
                "--db",
                "/private/var/kanade-rust-e2e/db.sqlite3",
                "--lock-dir",
                "/private/var/kanade-rust-e2e/locks",
                "--timezone",
                "Asia/Kuala_Lumpur",
                "--reset-weekday",
                "wed",
                "--reset-time",
                "08:00",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .unwrap()
    }

    #[test]
    fn requires_complete_absolute_harness_arguments() {
        assert!(parse(["--db", "relative.sqlite"].into_iter().map(str::to_owned)).is_err());
        assert!(
            parse(
                [
                    "--db",
                    "/a",
                    "--lock-dir",
                    "/b",
                    "--timezone",
                    "UTC",
                    "--reset-weekday",
                    "wed",
                    "--reset-time",
                    "8:00"
                ]
                .into_iter()
                .map(str::to_owned)
            )
            .is_err()
        );
    }

    #[test]
    fn policy_keeps_fresh_server_defaults_except_its_reset_seed() {
        let args = args();
        let defaults = RuntimeSettings::default();
        let policy = policy(&args);
        assert_eq!(policy.reminders.ping_time, defaults.pings.day_of_ping_time);
        assert_eq!(
            policy.reminders.countdowns,
            defaults.pings.countdown_minutes
        );
        assert_eq!(policy.reset_weekday, Weekday::Wed);
        assert_eq!(policy.reset_time, NaiveTime::from_hms_opt(8, 0, 0).unwrap());
    }

    #[test]
    fn picks_a_future_slot_in_the_next_boss_week() {
        let policy = policy(&args());
        let now = DateTime::from_timestamp(1_791_676_800, 0).unwrap();
        let run = next_run(now, &policy).unwrap();
        assert!(run.datetime > now);
        assert_eq!(
            utc_instant(&policy.week_of(&run.datetime).unwrap()).unwrap(),
            run.week_start
        );
        assert_eq!(run.participants, Vec::<String>::new());
        assert_eq!(run.channel_id, None);
    }
}
