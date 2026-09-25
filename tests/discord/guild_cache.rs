//! The guild cache fed by the router, read through the API channel list, the
//! chat gate's directory and the delivery reachability check.

use std::collections::BTreeSet;
use std::sync::Arc;

use twilight_model::id::Id;

use kanade::api::state::{ChannelEntry, ChannelList};
use kanade::bot::events::Router;
use kanade::bot::guild_cache::{GuildCache, WatchList};
use kanade::chat::gate::{ChannelDirectory, ChannelInfo};
use kanade::domain::notify::ChannelDirectory as PostDirectory;

use super::support::*;

const CATEGORY_ID: u64 = 350;
const WATCHED: u64 = 351;
const LOCKED: u64 = 352;
const MUTED: u64 = 353;
const BOT_ALLOWED: u64 = 354;
const THREAD: u64 = 355;
const BOT_ROLE: u64 = 510;

fn member(user: u64, roles: &[u64]) -> serde_json::Value {
    member_json(user_json(user, "kanade", None, true), None, roles)
}

/// `@everyone` may view and send; `LOCKED` hides itself from `@everyone`,
/// `MUTED` denies sending to the bot's role, `BOT_ALLOWED` hides from
/// everyone but lets the bot's own member in.
fn fed(self_member: bool) -> (Router, Arc<GuildCache>) {
    let cache = Arc::new(GuildCache::new(guild()));
    let mut router = Router::with_cache(scope(), Arc::clone(&cache));
    router.route(ready(SELF_ID));
    let members = if self_member {
        vec![member(SELF_ID, &[BOT_ROLE])]
    } else {
        Vec::new()
    };
    router.route(guild_create_with(
        GUILD,
        OWNER,
        &[
            role_json(GUILD, VIEW_CHANNEL | SEND_MESSAGES | SEND_IN_THREADS),
            role_json(BOT_ROLE, 0),
        ],
        &[
            channel_json(CATEGORY_ID, CATEGORY, "bossing", None, &[]),
            channel_json(WATCHED, TEXT, "kalos", Some(CATEGORY_ID), &[]),
            channel_json(
                LOCKED,
                TEXT,
                "staff",
                None,
                &[overwrite(GUILD, 0, 0, VIEW_CHANNEL)],
            ),
            channel_json(
                MUTED,
                TEXT,
                "announcements",
                None,
                &[overwrite(BOT_ROLE, 0, 0, SEND_MESSAGES)],
            ),
            channel_json(
                BOT_ALLOWED,
                TEXT,
                "bot-home",
                None,
                &[
                    overwrite(GUILD, 0, 0, VIEW_CHANNEL),
                    overwrite(SELF_ID, 1, VIEW_CHANNEL, 0),
                ],
            ),
        ],
        &[thread_json(THREAD, WATCHED, "tonight")],
        &members,
    ));
    (router, cache)
}

#[test]
fn chat_directory_describes_channels_and_threads() {
    let (_, cache) = fed(true);
    assert_eq!(
        ChannelDirectory::channel(&*cache, &WATCHED.to_string()),
        Some(ChannelInfo {
            id: WATCHED.to_string(),
            name: Some("kalos".into()),
            category_id: Some(CATEGORY_ID.to_string()),
            parent_id: None,
        })
    );
    assert_eq!(
        ChannelDirectory::channel(&*cache, &THREAD.to_string()),
        Some(ChannelInfo {
            id: THREAD.to_string(),
            name: Some("tonight".into()),
            category_id: None,
            parent_id: Some(WATCHED.to_string()),
        })
    );
    assert_eq!(ChannelDirectory::channel(&*cache, "999"), None);
    assert_eq!(ChannelDirectory::channel(&*cache, "0351"), None);
}

#[test]
fn channel_names_resolve_by_id_text() {
    let (_, cache) = fed(true);
    assert_eq!(
        cache.channel_name(&WATCHED.to_string()).as_deref(),
        Some("kalos")
    );
    assert_eq!(
        cache.channel_name(&THREAD.to_string()).as_deref(),
        Some("tonight")
    );
    assert_eq!(cache.channel_name("999"), None);
    assert_eq!(cache.channel_name("not-an-id"), None);
}

#[test]
fn channel_list_holds_text_channels_with_the_watch_flag() {
    let (_, cache) = fed(true);
    cache.set_watch(WatchList {
        channel_ids: BTreeSet::from([Id::new(MUTED)]),
        category_ids: BTreeSet::from([Id::new(CATEGORY_ID)]),
    });
    let entries = ChannelList::channels(&*cache);
    let entry = |id: u64, name: &str, watched: bool| ChannelEntry {
        id: id.to_string(),
        name: name.into(),
        watched,
    };
    assert_eq!(
        entries,
        vec![
            entry(WATCHED, "#kalos", true),
            entry(LOCKED, "#staff", false),
            entry(MUTED, "#announcements", true),
            entry(BOT_ALLOWED, "#bot-home", false),
        ],
        "no category or thread; same position sorts by id"
    );
    assert!(
        cache.is_watched(Id::new(THREAD)),
        "a thread counts as its parent"
    );
    assert!(!cache.is_watched(Id::new(999)));
}

#[test]
fn reachability_follows_the_bots_computed_permissions() {
    let (_, cache) = fed(true);
    assert!(cache.is_reachable(&WATCHED.to_string()));
    assert!(
        !cache.is_reachable(&LOCKED.to_string()),
        "@everyone overwrite hides it"
    );
    assert!(
        !cache.is_reachable(&MUTED.to_string()),
        "role overwrite denies send"
    );
    assert!(
        cache.is_reachable(&BOT_ALLOWED.to_string()),
        "member overwrite wins"
    );
    assert!(
        cache.is_reachable(&THREAD.to_string()),
        "threads use the parent"
    );
    assert!(
        !cache.is_reachable(&CATEGORY_ID.to_string()),
        "not text-capable"
    );
    assert!(
        !cache.is_reachable("999"),
        "unknown channels are unreachable"
    );
}

#[test]
fn administrator_and_role_changes_are_applied() {
    let (mut router, cache) = fed(true);
    router.route(role_update(GUILD, role_json(BOT_ROLE, ADMINISTRATOR)));
    assert!(cache.is_reachable(&LOCKED.to_string()));
    assert!(cache.is_reachable(&MUTED.to_string()));
    router.route(member_update(
        GUILD,
        user_json(SELF_ID, "kanade", None, true),
        None,
        &[],
    ));
    assert!(
        !cache.is_reachable(&LOCKED.to_string()),
        "the bot lost the role"
    );
}

#[test]
fn unknown_own_permissions_count_as_reachable_like_v4() {
    let (_, cache) = fed(false);
    assert_eq!(cache.permissions(Id::new(WATCHED)), None);
    assert!(cache.is_reachable(&LOCKED.to_string()));
    assert!(!cache.is_reachable("999"));
}

#[test]
fn channel_and_thread_events_keep_the_cache_current() {
    let (mut router, cache) = fed(true);
    assert_eq!(
        router.route(channel_create(
            GUILD,
            channel_json(360, TEXT, "new", Some(CATEGORY_ID), &[])
        )),
        None
    );
    assert_eq!(cache.channel_name("360").as_deref(), Some("new"));
    router.route(channel_update(
        GUILD,
        channel_json(360, TEXT, "renamed", None, &[]),
    ));
    assert_eq!(cache.channel_name("360").as_deref(), Some("renamed"));

    router.route(thread_update(GUILD, thread_json(THREAD, WATCHED, "moved")));
    assert_eq!(
        cache.channel_name(&THREAD.to_string()).as_deref(),
        Some("moved")
    );

    router.route(channel_delete(
        GUILD,
        channel_json(WATCHED, TEXT, "kalos", Some(CATEGORY_ID), &[]),
    ));
    assert_eq!(cache.channel_name(&WATCHED.to_string()), None);
    assert_eq!(
        cache.channel_name(&THREAD.to_string()),
        None,
        "a deleted channel takes its threads"
    );
}

#[test]
fn thread_list_sync_replaces_the_synced_parents_threads() {
    let (mut router, cache) = fed(true);
    router.route(thread_create(GUILD, thread_json(370, MUTED, "keep")));
    router.route(thread_list_sync(
        GUILD,
        &[WATCHED],
        &[thread_json(371, WATCHED, "fresh")],
    ));
    assert_eq!(cache.channel_name(&THREAD.to_string()), None);
    assert_eq!(cache.channel_name("371").as_deref(), Some("fresh"));
    assert_eq!(
        cache.channel_name("370").as_deref(),
        Some("keep"),
        "other parents untouched"
    );

    router.route(thread_list_sync(GUILD, &[], &[]));
    assert_eq!(
        cache.channel_name("370"),
        None,
        "an unscoped sync covers every parent"
    );
    assert_eq!(cache.channel_name("371"), None);
}

#[test]
fn nothing_is_known_before_the_guild_is_available() {
    let cache = Arc::new(GuildCache::new(guild()));
    let mut router = Router::with_cache(scope(), Arc::clone(&cache));
    assert!(!cache.is_available());
    router.route(channel_create(
        OTHER_GUILD,
        channel_json(WATCHED, TEXT, "kalos", None, &[]),
    ));
    assert!(ChannelList::channels(&*cache).is_empty());
    assert!(!cache.is_reachable(&WATCHED.to_string()));
    let (_, available) = fed(true);
    assert!(available.is_available());
}

#[test]
fn owner_bot_has_every_permission() {
    let cache = Arc::new(GuildCache::new(guild()));
    let mut router = Router::with_cache(scope(), Arc::clone(&cache));
    router.route(ready(SELF_ID));
    router.route(guild_create_with(
        GUILD,
        SELF_ID,
        &[role_json(GUILD, 0)],
        &[channel_json(
            LOCKED,
            TEXT,
            "staff",
            None,
            &[overwrite(GUILD, 0, 0, VIEW_CHANNEL)],
        )],
        &[],
        &[member(SELF_ID, &[])],
    ));
    assert!(cache.is_reachable(&LOCKED.to_string()));
}
