//! The §2.1 base world (all invented): ids, the T0 clock, the seeded store
//! and the gateway payloads in Discord's JSON shapes.

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use kanade::{
    bot::{delivery::FixedClock, extract_feed::snowflake_before},
    domain::{
        history::Origin,
        ids::IdGenerator,
        members::{GatewayMember, MemberStore},
        model_log::{ModelLogStore, WatchedMessage},
        notify::WeekReset,
        schedule::{NewRun, RsvpSource, RsvpState, RunSource, RunStatus, SchedulePolicy},
        scheduler::SchedulerService,
    },
    infrastructure::store::SqliteStore,
};
use serde_json::{Value, json};
use twilight_gateway::Event;
use twilight_model::gateway::payload::incoming::{
    GuildCreate, MessageCreate, MessageUpdate, Ready,
};

pub const ZONE: Tz = chrono_tz::Asia::Singapore;
pub const GUILD: u64 = 100_000_000_000_000_900;
/// The bossing role every member holds; also the chat pilot role.
pub const BOSSING: u64 = 100_000_000_000_000_300;
pub const CATEGORY: u64 = 100_000_000_000_000_200;
pub const C1: u64 = 100_000_000_000_000_201;
pub const C2: u64 = 100_000_000_000_000_202;
pub const BOT: u64 = 100_000_000_000_000_001;
pub const BOT_NAME: &str = "Kanade";
/// The guild owner is nobody in the fixture: an owner is staff, and staff
/// skip chat limits and pass authority checks.
pub const OWNER: u64 = 100_000_000_000_000_950;

pub const ASTER: u64 = 100_000_000_000_000_101;
pub const BRAMBLE: u64 = 100_000_000_000_000_102;
pub const COBALT: u64 = 100_000_000_000_000_103;
pub const DUNE: u64 = 100_000_000_000_000_104;
pub const FENNEL: u64 = 100_000_000_000_000_105;
pub const MEMBERS: [(u64, &str); 5] = [
    (ASTER, "Aster"),
    (BRAMBLE, "Bramble"),
    (COBALT, "Cobalt"),
    (DUNE, "Dune"),
    (FENNEL, "Fennel"),
];

pub fn name_of(id: u64) -> &'static str {
    MEMBERS
        .iter()
        .find(|(member, _)| *member == id)
        .map_or("someone", |(_, name)| name)
}

/// Guild-local wall time on a day of October 2026.
pub fn local(day: u32, hour: u32, minute: u32) -> DateTime<Utc> {
    let date = NaiveDate::from_ymd_opt(2026, 10, day).expect("an October date");
    let time = NaiveTime::from_hms_opt(hour, minute, 0).expect("a wall time");
    ZONE.from_local_datetime(&date.and_time(time))
        .single()
        .expect("Singapore has no gaps")
        .with_timezone(&Utc)
}

/// T0 = Tue 13 Oct 2026 15:30 SGT.
pub fn t0() -> DateTime<Utc> {
    local(13, 15, 30)
}

/// The `n`th message id created at `at` (ordered like Discord snowflakes).
pub fn snowflake(at: DateTime<Utc>, n: u64) -> u64 {
    snowflake_before(at).get() + 1 + n
}

/// One seeded run (§2.1).
pub struct SeedRun {
    pub short: &'static str,
    pub bosses: &'static [&'static str],
    pub at: (u32, u32, u32),
    pub channel: u64,
    pub party: &'static [u64],
    pub status: RunStatus,
}

pub const RUNS: [SeedRun; 8] = [
    SeedRun {
        short: "a1000001",
        bosses: &["HMaleficStar", "HFA"],
        at: (12, 21, 30),
        channel: C1,
        party: &[ASTER, BRAMBLE, COBALT],
        status: RunStatus::Done,
    },
    SeedRun {
        short: "a1000002",
        bosses: &["HCarling"],
        at: (13, 21, 30),
        channel: C1,
        party: &[ASTER, BRAMBLE],
        status: RunStatus::Planned,
    },
    SeedRun {
        short: "a1000003",
        bosses: &["HMaleficStar"],
        at: (14, 21, 30),
        channel: C1,
        party: &[ASTER, COBALT, DUNE],
        status: RunStatus::Planned,
    },
    SeedRun {
        short: "b2000005",
        bosses: &["NBaldrix"],
        at: (14, 22, 0),
        channel: C2,
        party: &[DUNE, FENNEL],
        status: RunStatus::Planned,
    },
    SeedRun {
        short: "a1000004",
        bosses: &["XKalos"],
        at: (14, 23, 0),
        channel: C1,
        party: &[BRAMBLE, COBALT],
        status: RunStatus::Planned,
    },
    // The draft has this run come from weekly `f1000001`; it is seeded as a
    // one-off run instead (see the receipt's Decision required: serve's tick
    // would materialise that weekly into W43 and change C02).
    SeedRun {
        short: "a1000006",
        bosses: &["HLimbo"],
        at: (16, 22, 0),
        channel: C1,
        party: &[ASTER, FENNEL],
        status: RunStatus::Planned,
    },
    SeedRun {
        short: "a1000008",
        bosses: &["HFA"],
        at: (17, 21, 0),
        channel: C1,
        party: &[ASTER, COBALT],
        status: RunStatus::Cancelled,
    },
    SeedRun {
        short: "b2000007",
        bosses: &["HBellona"],
        at: (19, 21, 0),
        channel: C2,
        party: &[COBALT, FENNEL],
        status: RunStatus::Planned,
    },
];

/// The short id as a full row id (UUID-shaped, so prefixes resolve as usual).
pub fn full_id(short: &str) -> String {
    format!("{short}-0000-4000-8000-000000000000")
}

/// Hands out one chosen id, then random ones (history rows and the like).
struct FirstId(Option<String>);

impl IdGenerator for FirstId {
    fn new_id(&mut self) -> String {
        self.0
            .take()
            .unwrap_or_else(|| uuid::Uuid::new_v4().hyphenated().to_string())
    }
}

/// A message seeded as already read before the case (E09's context line).
pub struct ContextMessage {
    pub author: u64,
    pub channel: u64,
    pub at: DateTime<Utc>,
    pub text: &'static str,
}

/// Seed the §2.1 world through production store APIs; errors name the step.
pub async fn seed(
    store: &SqliteStore,
    policy: &SchedulePolicy,
    context: &[ContextMessage],
) -> Result<(), String> {
    for (id, name) in MEMBERS {
        store
            .apply_gateway(GatewayMember {
                user_id: id.to_string(),
                display_name: Some(name.to_owned()),
                has_role: true,
                roles: vec![BOSSING.to_string()],
                ..GatewayMember::default()
            })
            .await
            .map_err(|error| format!("member {name}: {error}"))?;
    }
    let reset = WeekReset {
        zone: policy.zone(),
        weekday: policy.reset_weekday,
        time: policy.reset_time,
    };
    for run in &RUNS {
        let at = local(run.at.0, run.at.1, run.at.2);
        let week_start = reset
            .current_week(at)
            .map_err(|_| format!("week of {}", run.short))?;
        let id = SchedulerService::new(store, FirstId(Some(full_id(run.short))), FixedClock(t0()))
            .with_attendance(policy.attendance)
            .as_origin(Origin::for_tests())
            .create_run(NewRun {
                fixed_run_id: None,
                channel_id: Some(run.channel.to_string()),
                week_start,
                datetime: at,
                bosses: run.bosses.iter().map(|boss| (*boss).to_owned()).collect(),
                participants: run.party.iter().map(u64::to_string).collect(),
                status: run.status,
                source: RunSource::Amend,
            })
            .await
            .map_err(|error| format!("run {}: {error}", run.short))?;
        if !id.starts_with(run.short) {
            return Err(format!("run {} was stored as {id}", run.short));
        }
    }
    SchedulerService::new(store, FirstId(None), FixedClock(t0()))
        .with_attendance(policy.attendance)
        .as_origin(Origin::for_tests())
        .set_rsvp(
            &full_id("a1000002"),
            &ASTER.to_string(),
            RsvpState::Yes,
            RsvpSource::Reaction,
        )
        .await
        .map_err(|error| format!("Aster's RSVP: {error}"))?;
    for (n, line) in context.iter().enumerate() {
        let id = snowflake(line.at, 500 + n as u64).to_string();
        store
            .upsert_message(WatchedMessage {
                id: id.clone(),
                channel_id: line.channel.to_string(),
                author_id: line.author.to_string(),
                created_at: line.at,
                edited_at: None,
                content: line.text.to_owned(),
                processed_at: None,
            })
            .await
            .map_err(|error| format!("context message: {error}"))?;
        store
            .mark_processed(&[id], line.at)
            .await
            .map_err(|error| format!("context message read: {error}"))?;
    }
    Ok(())
}

// ---- Gateway payloads ----

fn parse<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("a Discord payload fixture")
}

fn user_json(id: u64, name: &str, bot: bool) -> Value {
    json!({
        "id": id.to_string(), "username": name, "global_name": null,
        "discriminator": "0", "avatar": null, "bot": bot,
    })
}

pub fn member_json(id: u64, name: &str) -> Value {
    json!({
        "user": user_json(id, name, false),
        "nick": null,
        "roles": [BOSSING.to_string()],
        "joined_at": null, "deaf": false, "mute": false, "flags": 0,
        "communication_disabled_until": null,
    })
}

pub fn guild_members() -> Vec<twilight_model::guild::Member> {
    MEMBERS
        .iter()
        .map(|(id, name)| parse(member_json(*id, name)))
        .collect()
}

pub fn ready() -> Event {
    Event::Ready(parse::<Ready>(json!({
        "application": { "id": "9", "flags": 0 },
        "guilds": [],
        "resume_gateway_url": "wss://gateway.invalid",
        "session_id": "session",
        "user": {
            "id": BOT.to_string(), "username": BOT_NAME, "discriminator": "0",
            "avatar": null, "bot": true, "mfa_enabled": false,
        },
        "v": 10,
    })))
}

fn role_json(id: u64, name: &str) -> Value {
    json!({
        "color": 0,
        "colors": { "primary_color": 0, "secondary_color": null, "tertiary_color": null },
        "hoist": false, "id": id.to_string(), "managed": false, "mentionable": false,
        "name": name, "permissions": "0", "position": 1, "flags": 0,
    })
}

fn channel_json(id: u64, kind: u8, name: &str, parent: Option<u64>) -> Value {
    json!({
        "id": id.to_string(), "type": kind, "name": name,
        "parent_id": parent.map(|id| id.to_string()), "position": 0,
        "permission_overwrites": [],
    })
}

/// The guild with the bossing category and `#boss-alpha` / `#boss-beta`.
pub fn guild_create() -> Event {
    let mut guild = json!({
        "afk_channel_id": null, "afk_timeout": 300, "application_id": null, "banner": null,
        "default_message_notifications": 0, "description": null, "discovery_splash": null,
        "emojis": [], "explicit_content_filter": 0, "features": [], "icon": null,
        "id": GUILD.to_string(), "mfa_level": 0, "name": "v02-guild", "nsfw_level": 0,
        "owner_id": OWNER.to_string(), "preferred_locale": "en-US",
        "premium_progress_bar_enabled": false, "premium_tier": 0,
        "public_updates_channel_id": null,
        "roles": [role_json(GUILD, "@everyone"), role_json(BOSSING, "bossing")],
        "rules_channel_id": null, "splash": null, "system_channel_flags": 0,
        "system_channel_id": null, "verification_level": 0, "vanity_url_code": null,
    });
    for list in [
        "presences",
        "stickers",
        "voice_states",
        "threads",
        "members",
        "guild_scheduled_events",
        "stage_instances",
    ] {
        guild[list] = json!([]);
    }
    guild["channels"] = json!([
        channel_json(CATEGORY, 4, "bossing", None),
        channel_json(C1, 0, "boss-alpha", Some(CATEGORY)),
        channel_json(C2, 0, "boss-beta", Some(CATEGORY)),
    ]);
    Event::GuildCreate(Box::new(parse::<GuildCreate>(guild)))
}

fn timestamp(at: DateTime<Utc>) -> String {
    twilight_model::util::Timestamp::from_micros(at.timestamp_micros())
        .expect("a valid instant")
        .iso_8601()
        .to_string()
}

/// User ids mentioned as `<@id>` / `<@!id>` in `text`.
fn mentioned(text: &str) -> Vec<u64> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        let tail = rest[start + 2..].trim_start_matches('!');
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if tail[digits.len()..].starts_with('>')
            && let Ok(id) = digits.parse()
        {
            ids.push(id);
        }
        rest = &rest[start + 2..];
    }
    ids
}

pub fn message_json(
    id: u64,
    channel: u64,
    author: u64,
    content: &str,
    at: DateTime<Utc>,
    edited: Option<DateTime<Utc>>,
) -> Value {
    let mentions: Vec<Value> = mentioned(content)
        .into_iter()
        .map(|user| {
            let mut value = user_json(user, name_of(user), user == BOT);
            value["public_flags"] = json!(0);
            if user == BOT {
                value["username"] = json!(BOT_NAME);
            }
            value
        })
        .collect();
    json!({
        "id": id.to_string(),
        "channel_id": channel.to_string(),
        "guild_id": GUILD.to_string(),
        "author": user_json(author, name_of(author), false),
        "member": {
            "roles": [BOSSING.to_string()],
            "joined_at": "2026-01-01T00:00:00.000000+00:00",
            "deaf": false, "mute": false, "flags": 0,
        },
        "content": content,
        "timestamp": timestamp(at),
        "edited_timestamp": edited.map(timestamp),
        "tts": false,
        "mention_everyone": false,
        "mentions": mentions,
        "mention_roles": [],
        "attachments": [],
        "embeds": [],
        "pinned": false,
        "type": 0,
    })
}

pub fn posted(value: Value) -> Event {
    Event::MessageCreate(Box::new(parse::<MessageCreate>(value)))
}

pub fn edited(value: Value) -> Event {
    Event::MessageUpdate(Box::new(parse::<MessageUpdate>(value)))
}
