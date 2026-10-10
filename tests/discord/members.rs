//! Member events as roster updates.

use kanade::bot::events::{BotEvent, RosterUpdate};
use kanade::domain::members::{Directory, Member, PingLevel, Roster};

use super::support::*;

fn roster_event(event: twilight_gateway::Event) -> Option<RosterUpdate> {
    match route(scope(), event)? {
        BotEvent::Roster(update) => Some(update),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn joining_with_the_role_is_seen_with_discord_display_name() {
    let member = member_json(
        user_json(ALICE, "alice", Some("Alice G"), false),
        Some("Ali"),
        &[BOSSING_ROLE, 7],
    );
    assert_eq!(
        roster_event(member_add(GUILD, member)),
        Some(RosterUpdate::Seen {
            user_id: ALICE.to_string(),
            display_name: "Ali".into(),
            nickname: Some("Ali".into()),
            has_role: true,
            roles: vec![BOSSING_ROLE.to_string(), "7".into()],
            is_guild_admin: false,
        })
    );
}

#[test]
fn display_name_falls_back_to_global_then_username() {
    let global = member_json(user_json(ALICE, "alice", Some("Alice G"), false), None, &[]);
    let Some(RosterUpdate::Seen {
        display_name,
        has_role,
        ..
    }) = roster_event(member_add(GUILD, global))
    else {
        panic!("seen");
    };
    assert_eq!((display_name.as_str(), has_role), ("Alice G", false));

    let plain = member_json(user_json(ALICE, "alice", Some(""), false), Some(""), &[]);
    let Some(RosterUpdate::Seen { display_name, .. }) = roster_event(member_add(GUILD, plain))
    else {
        panic!("seen");
    };
    assert_eq!(display_name, "alice");
}

#[test]
fn losing_the_role_and_leaving_update_the_roster() {
    let mut roster = Roster::new();
    roster.upsert(Member {
        user_id: ALICE.to_string(),
        display_name: Some("alice".into()),
        has_role: true,
        ping_level: PingLevel::All,
        ..Member::default()
    });

    let update = roster_event(member_update(
        GUILD,
        user_json(ALICE, "alice", None, false),
        Some("Ali"),
        &[7],
    ))
    .unwrap();
    update.apply(&mut roster);
    let alice = roster.member(&ALICE.to_string()).unwrap();
    assert!(!alice.has_role);
    assert_eq!(alice.nickname.as_deref(), Some("Ali"));
    assert_eq!(alice.ping_level, PingLevel::All, "ping level is preserved");

    let regained = roster_event(member_update(
        GUILD,
        user_json(ALICE, "alice", None, false),
        None,
        &[BOSSING_ROLE],
    ))
    .unwrap();
    regained.apply(&mut roster);
    assert!(roster.member(&ALICE.to_string()).unwrap().has_role);

    let left = roster_event(member_remove(GUILD, user_json(ALICE, "alice", None, false))).unwrap();
    assert_eq!(left.user_id(), ALICE.to_string());
    left.apply(&mut roster);
    let alice = roster.member(&ALICE.to_string()).unwrap();
    assert!(!alice.has_role, "left members keep their row, not the role");
    assert_eq!(alice.ping_level, PingLevel::All);
}

#[test]
fn bots_and_other_guilds_produce_nothing() {
    let bot = member_json(user_json(BOB, "helper", None, true), None, &[BOSSING_ROLE]);
    assert_eq!(roster_event(member_add(GUILD, bot.clone())), None);
    assert_eq!(
        roster_event(member_remove(GUILD, user_json(BOB, "helper", None, true))),
        None
    );
    let alice = member_json(
        user_json(ALICE, "alice", None, false),
        None,
        &[BOSSING_ROLE],
    );
    assert_eq!(route(scope(), member_add(OTHER_GUILD, alice)), None);
}
