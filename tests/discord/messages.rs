//! Message, channel and thread events: guild scoping, drop counters and
//! thread-parent resolution.

use twilight_model::id::Id;

use kanade::bot::events::{BotEvent, DeletedMessages, DroppedCounts, Router};

use super::support::*;

const THREAD: u64 = 310;
const OTHER_CHANNEL: u64 = 320;

/// A router whose guild has one text channel with one active thread.
fn guild_router() -> Router {
    let mut router = Router::new(scope());
    router.route(ready(SELF_ID));
    router.route(guild_create_with(
        GUILD,
        OWNER,
        &[role_json(GUILD, VIEW_CHANNEL | SEND_MESSAGES)],
        &[channel_json(CHANNEL, TEXT, "kalos", None, &[])],
        &[thread_json(THREAD, CHANNEL, "tonight")],
        &[],
    ));
    router
}

fn created(event: Option<BotEvent>) -> (u64, u64, Option<u64>) {
    match event {
        Some(BotEvent::MessageCreated(message) | BotEvent::MessageUpdated(message)) => (
            message.message.id.get(),
            message.origin_channel_id.get(),
            message.thread_id.map(Id::get),
        ),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn a_guild_message_is_routed_with_its_own_channel_as_origin() {
    let mut router = guild_router();
    let event = router.route(message_create(message_json(
        1,
        CHANNEL,
        Some(GUILD),
        "kalos at 9?",
    )));
    let Some(BotEvent::MessageCreated(message)) = &event else {
        panic!("unexpected {event:?}");
    };
    assert_eq!(message.message.content, "kalos at 9?");
    assert_eq!(created(event), (1, CHANNEL, None));
}

#[test]
fn a_thread_message_groups_under_its_parent_channel() {
    let mut router = guild_router();
    assert_eq!(
        created(router.route(message_create(message_json(
            2,
            THREAD,
            Some(GUILD),
            "in thread"
        )))),
        (2, CHANNEL, Some(THREAD))
    );
    assert_eq!(
        created(router.route(message_update(message_json(
            2,
            THREAD,
            Some(GUILD),
            "edited"
        )))),
        (2, CHANNEL, Some(THREAD)),
        "edits resolve the same way"
    );
}

#[test]
fn threads_created_later_resolve_and_deleted_ones_do_not() {
    let mut router = guild_router();
    assert_eq!(
        router.route(thread_create(GUILD, thread_json(311, CHANNEL, "new"))),
        None
    );
    let message = || message_create(message_json(3, 311, Some(GUILD), "hi"));
    assert_eq!(created(router.route(message())), (3, CHANNEL, Some(311)));
    assert_eq!(router.route(thread_delete(GUILD, 311, CHANNEL)), None);
    assert_eq!(
        created(router.route(message())),
        (3, 311, None),
        "an unknown channel is its own origin (v4 origin_ids)"
    );
}

#[test]
fn deletes_and_bulk_deletes_are_routed_with_their_origin() {
    let mut router = guild_router();
    assert_eq!(
        router.route(message_delete(Some(GUILD), THREAD, 4)),
        Some(BotEvent::MessagesDeleted(DeletedMessages {
            channel_id: Id::new(THREAD),
            origin_channel_id: Id::new(CHANNEL),
            thread_id: Some(Id::new(THREAD)),
            message_ids: vec![Id::new(4)],
        }))
    );
    assert_eq!(
        router.route(message_delete_bulk(Some(GUILD), CHANNEL, &[5, 6])),
        Some(BotEvent::MessagesDeleted(DeletedMessages {
            channel_id: Id::new(CHANNEL),
            origin_channel_id: Id::new(CHANNEL),
            thread_id: None,
            message_ids: vec![Id::new(5), Id::new(6)],
        }))
    );
}

#[test]
fn other_guild_events_are_dropped_and_counted() {
    let mut router = guild_router();
    let dropped = router.dropped();
    for event in [
        message_create(message_json(7, OTHER_CHANNEL, Some(OTHER_GUILD), "x")),
        message_update(message_json(7, OTHER_CHANNEL, Some(OTHER_GUILD), "y")),
        message_delete(Some(OTHER_GUILD), OTHER_CHANNEL, 7),
        message_delete_bulk(Some(OTHER_GUILD), OTHER_CHANNEL, &[7, 8]),
        channel_create(
            OTHER_GUILD,
            channel_json(OTHER_CHANNEL, TEXT, "elsewhere", None, &[]),
        ),
        thread_create(OTHER_GUILD, thread_json(330, OTHER_CHANNEL, "t")),
        thread_delete(OTHER_GUILD, 330, OTHER_CHANNEL),
        thread_list_sync(OTHER_GUILD, &[], &[]),
    ] {
        assert_eq!(router.route(event), None);
    }
    assert_eq!(
        dropped.counts(),
        DroppedCounts {
            other_guild: 8,
            no_guild: 0
        }
    );
    assert!(
        router.cache().channel(Id::new(OTHER_CHANNEL)).is_none(),
        "other guilds never reach the cache"
    );
    assert_eq!(router.cache().channels().len(), 2);
}

#[test]
fn direct_messages_are_dropped_and_counted() {
    let mut router = guild_router();
    for event in [
        message_create(message_json(9, 999, None, "dm")),
        message_update(message_json(9, 999, None, "dm edit")),
        message_delete(None, 999, 9),
        message_delete_bulk(None, 999, &[9]),
        twilight_gateway::Event::InteractionCreate(Box::new(
            twilight_model::gateway::payload::incoming::InteractionCreate(debug_status(
                None,
                ALICE,
                &[],
                0,
            )),
        )),
    ] {
        assert_eq!(router.route(event), None);
    }
    assert_eq!(
        router.dropped().counts(),
        DroppedCounts {
            other_guild: 0,
            no_guild: 5
        }
    );
    router.route(twilight_gateway::Event::GatewayHeartbeatAck);
    assert_eq!(
        router.dropped().counts().no_guild,
        5,
        "gateway control events are not counted"
    );
}

#[test]
fn message_debug_omits_the_content() {
    let mut router = guild_router();
    let event = router.route(message_create(message_json(
        10,
        CHANNEL,
        Some(GUILD),
        "private words",
    )));
    let debug = format!("{event:?}");
    assert!(!debug.contains("private words"));
    assert!(debug.contains("content_len"));
}
