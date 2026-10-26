//! The `web` component alone with a bot token: the guild read over REST
//! (`discord::rest`) into the cache the API lists, and the roster brought in
//! line, sessions included, without a gateway.

use twilight_model::guild::Guild;
use twilight_model::user::CurrentUser;

use super::*;
use crate::{
    infrastructure::store::web_sessions::{
        LoginMethod, SessionOrigin, WebSession, WebSessionStore,
    },
    runtime::serve::{bot::discord::rest::RestGuild, budget::ShutdownClock},
};

/// Long enough that only the startup pass runs during a test.
const NEVER_AGAIN: Duration = Duration::from_secs(3600);

fn web_only() -> Harness {
    Harness::with(&[
        ("KANADE_COMPONENTS", "web"),
        ("KANADE_MODULE_GUIDES", "1"),
        ("KANADE_PUBLIC_HOST", "portal.kanade.test"),
        ("KANADE_PUBLIC_BIND", "127.0.0.1:0"),
        ("KANADE_CLOUDFLARED_PEER", "127.0.0.1"),
    ])
}

/// The guild as `GET /guilds/{id}` returns it: owner and roles, no
/// channels or members.
fn rest_guild() -> Guild {
    let Event::GuildCreate(create) = guild_create(&[]) else {
        unreachable!();
    };
    let GuildCreate::Available(guild) = *create else {
        unreachable!();
    };
    guild
}

fn bot_user() -> CurrentUser {
    parse(json!({
        "id": SELF.to_string(), "username": "kanade", "discriminator": "0",
        "avatar": null, "bot": true, "mfa_enabled": false,
    }))
}

fn web_session(fill: char, origin: SessionOrigin, subject: u64) -> WebSession {
    let now = clock::system_now();
    WebSession {
        id_hash: fill.to_string().repeat(64),
        origin,
        method: LoginMethod::Discord,
        subject: subject.to_string(),
        display: format!("member {subject}"),
        created_at: now,
        last_seen_at: now,
        checked_at: now,
        expires_at: now + chrono::TimeDelta::hours(1),
        avatar_hash: None,
        device: None,
        client_tag: None,
        superseded_until: None,
    }
}

/// Alice (staff through the admin role) with an admin session, Bob and
/// Gone (bossing) each with a portal session.
async fn seeded(harness: &Harness) {
    harness
        .seed(async |store| {
            for member in [
                gateway_member(ALICE, "alice", &[BOSSING, ADMIN_ROLE]),
                gateway_member(BOB, "bob", &[BOSSING]),
                gateway_member(GONE, "gone", &[BOSSING]),
            ] {
                store.apply_gateway(member).await.unwrap();
            }
            for session in [
                web_session('a', SessionOrigin::Admin, ALICE),
                web_session('b', SessionOrigin::Public, BOB),
                web_session('c', SessionOrigin::Public, GONE),
            ] {
                store.put_session(&session, None).await.unwrap();
            }
        })
        .await;
    let fake = &harness.fake;
    fake.seed_current_user(bot_user());
    fake.seed_guild(rest_guild());
    fake.seed_channels(
        Id::new(GUILD),
        vec![channel_json(HOME_A)].into_iter().map(parse).collect(),
    );
    // Alice lost the admin role and Gone left while nobody watched.
    fake.seed_members(
        Id::new(GUILD),
        vec![
            guild_member(ALICE, "alice", false, &[BOSSING]),
            guild_member(BOB, "bob", false, &[BOSSING]),
            guild_member(SELF, "kanade", true, &[BOSSING]),
        ],
    );
}

async fn live(store: &SqliteStore, fill: char) -> bool {
    store
        .load_session(&fill.to_string().repeat(64))
        .await
        .unwrap()
        .is_some()
}

/// Compose as a web-only serve does, then start the reader over the fake.
async fn start(harness: &Harness) -> (Arc<SqliteStore>, Composition, RestGuild) {
    let store = store::open(&harness.config.store).await.unwrap();
    let cache = Arc::new(GuildCache::new(Id::new(GUILD)));
    let mut composition = api::compose(
        &harness.config,
        store.clone(),
        cache.clone(),
        LiveHealth::new(store.clone()),
    )
    .await
    .unwrap();
    let rest = RestGuild::start(
        &harness.config,
        store.clone(),
        &mut composition,
        cache,
        Arc::clone(&harness.fake),
        NEVER_AGAIN,
    )
    .await
    .unwrap();
    (store, composition, rest)
}

async fn stop(
    harness: &Harness,
    store: Arc<SqliteStore>,
    composition: Composition,
    rest: RestGuild,
) {
    rest.stop(&ShutdownClock::default()).await;
    drop(composition);
    store::close(store, Duration::ZERO).await;
    let reopened = store::open(&harness.config.store)
        .await
        .expect("every store handle was released");
    store::close(reopened, Duration::ZERO).await;
}

#[tokio::test]
async fn the_guild_is_read_over_rest_and_departures_end_sessions() {
    let harness = web_only();
    seeded(&harness).await;
    let (store, composition, rest) = start(&harness).await;
    eventually!(
        "the roster pass",
        !store
            .load_member(&GONE.to_string())
            .await
            .unwrap()
            .unwrap()
            .member
            .has_role
    );
    eventually!("alice's admin session ends", !live(&store, 'a').await);
    assert!(!live(&store, 'c').await, "gone's portal session ended");
    assert!(live(&store, 'b').await, "bob is still a bossing member");
    assert_eq!(roles_of(&store, ALICE).await.unwrap(), ["10"]);
    assert_eq!(composition.access.owner(), Some(Id::new(OWNER)));

    let state = &composition.admin.state;
    assert!(state.channels.connected());
    let listed: Vec<String> = state
        .channels
        .channels()
        .into_iter()
        .map(|channel| channel.id)
        .collect();
    assert_eq!(listed, [HOME_A.to_string()], "the channel pickers fill");
    assert_eq!(
        state.channels.bot_user_id(),
        Some(SELF.to_string()),
        "the bot's user"
    );
    assert!(state.application.is_some(), "Setup reads the application");
    assert!(
        !state.guides.as_ref().expect("module guides").publishing(),
        "guides never publish without the bot"
    );
    // Reads only: nothing is posted, registered or reacted.
    assert!(harness.fake.calls().iter().all(|call| matches!(
        call,
        Call::ApplicationEmojis { .. }
            | Call::ApplicationInfo { .. }
            | Call::CurrentUser { .. }
            | Call::Guild { .. }
            | Call::GuildChannels { .. }
            | Call::ListMembers { .. }
    )));
    stop(&harness, store, composition, rest).await;
}

#[tokio::test]
async fn a_failed_member_page_concludes_no_departures() {
    let harness = web_only();
    seeded(&harness).await;
    harness
        .fake
        .script(Op::ListMembers, Step::Reject(RejectionKind::MissingAccess));
    let (store, composition, rest) = start(&harness).await;
    eventually!("the failed page", harness.fake.count(Op::ListMembers) == 1);
    // The guild itself was read; the roster pass stopped at the page.
    eventually!("the guild", composition.admin.state.channels.connected());
    sleep(TICK * 4).await;
    let gone = store.load_member(&GONE.to_string()).await.unwrap().unwrap();
    assert!(gone.member.has_role, "never concluded from a failed page");
    for fill in ['a', 'b', 'c'] {
        assert!(live(&store, fill).await, "session {fill} kept");
    }
    assert_eq!(roles_of(&store, ALICE).await.unwrap(), ["10", "20"]);
    stop(&harness, store, composition, rest).await;
}
