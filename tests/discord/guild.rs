//! Guild owner and role permissions: computed Administrator on roster updates.

use kanade::bot::events::{AdminRoles, BotEvent, RosterUpdate, Router};

use super::support::*;

const MOD_ROLE: u64 = 502;
const SEND_MESSAGES: u64 = 1 << 11;

fn admin_roles(roles: &[u64], everyone: bool) -> AdminRoles {
    AdminRoles {
        everyone,
        roles: roles.iter().map(u64::to_string).collect(),
    }
}

fn available(owner: u64, admin: AdminRoles) -> Option<BotEvent> {
    Some(BotEvent::GuildAvailable {
        owner_id: user(owner),
        admin_roles: admin,
    })
}

/// Alice's computed Administrator after a member update with `roles`.
fn alice_admin(router: &mut Router, roles: &[u64]) -> bool {
    match router.route(member_update(
        GUILD,
        user_json(ALICE, "alice", None, false),
        None,
        roles,
    )) {
        Some(BotEvent::Roster(RosterUpdate::Seen {
            roles: seen,
            is_guild_admin,
            ..
        })) => {
            assert_eq!(seen, roles.iter().map(u64::to_string).collect::<Vec<_>>());
            is_guild_admin
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn guild_router() -> Router {
    let mut router = Router::new(scope());
    let created = router.route(guild_create(
        GUILD,
        OWNER,
        &[
            role_json(GUILD, SEND_MESSAGES),
            role_json(ADMIN_ROLE, ADMINISTRATOR | SEND_MESSAGES),
            role_json(MOD_ROLE, SEND_MESSAGES),
        ],
    ));
    assert_eq!(created, available(OWNER, admin_roles(&[ADMIN_ROLE], false)));
    router
}

#[test]
fn administrator_comes_from_the_members_role_permissions() {
    let mut router = guild_router();
    assert!(alice_admin(&mut router, &[BOSSING_ROLE, ADMIN_ROLE]));
    assert!(!alice_admin(&mut router, &[BOSSING_ROLE, MOD_ROLE]));
    assert!(
        !alice_admin(&mut router, &[9999]),
        "an unknown role never grants"
    );
}

#[test]
fn nothing_is_administrator_before_the_guild_is_available() {
    let mut router = Router::new(scope());
    assert!(!alice_admin(&mut router, &[ADMIN_ROLE]));
    assert_eq!(
        router.route(role_create(GUILD, role_json(MOD_ROLE, ADMINISTRATOR))),
        None,
        "no owner yet: guild access waits for GUILD_CREATE"
    );
}

#[test]
fn role_events_change_administrator_and_report_the_new_set() {
    let mut router = guild_router();

    assert_eq!(
        router.route(role_update(GUILD, role_json(MOD_ROLE, ADMINISTRATOR))),
        available(OWNER, admin_roles(&[ADMIN_ROLE, MOD_ROLE], false))
    );
    assert!(alice_admin(&mut router, &[MOD_ROLE]));

    assert_eq!(
        router.route(role_update(GUILD, role_json(MOD_ROLE, SEND_MESSAGES))),
        available(OWNER, admin_roles(&[ADMIN_ROLE], false)),
        "losing Administrator is reported so stored flags are revoked"
    );
    assert!(!alice_admin(&mut router, &[MOD_ROLE]));

    assert_eq!(
        router.route(role_update(GUILD, role_json(MOD_ROLE, SEND_MESSAGES | 1))),
        None,
        "a change that leaves Administrator alone is not reported"
    );

    assert_eq!(
        router.route(role_create(GUILD, role_json(503, ADMINISTRATOR))),
        available(OWNER, admin_roles(&[ADMIN_ROLE, 503], false))
    );
    assert_eq!(
        router.route(role_delete(GUILD, ADMIN_ROLE)),
        available(OWNER, admin_roles(&[503], false))
    );
    assert!(!alice_admin(&mut router, &[ADMIN_ROLE]));
}

#[test]
fn everyone_role_grants_every_member() {
    let mut router = guild_router();
    assert_eq!(
        router.route(role_update(GUILD, role_json(GUILD, ADMINISTRATOR))),
        available(OWNER, admin_roles(&[ADMIN_ROLE], true))
    );
    assert!(alice_admin(&mut router, &[]));
}

#[test]
fn guild_update_tracks_the_owner_and_roles() {
    let mut router = guild_router();
    let roles = [
        role_json(GUILD, 0),
        role_json(ADMIN_ROLE, ADMINISTRATOR),
        role_json(MOD_ROLE, SEND_MESSAGES),
    ];
    assert_eq!(router.route(guild_update(GUILD, OWNER, &roles)), None);
    assert_eq!(
        router.route(guild_update(GUILD, ALICE, &roles)),
        available(ALICE, admin_roles(&[ADMIN_ROLE], false))
    );
    assert_eq!(
        router.route(guild_update(GUILD, ALICE, &roles[..1])),
        available(ALICE, admin_roles(&[], false))
    );
}

#[test]
fn other_guilds_do_not_touch_the_cache() {
    let mut router = guild_router();
    assert_eq!(
        router.route(role_update(OTHER_GUILD, role_json(MOD_ROLE, ADMINISTRATOR))),
        None
    );
    assert_eq!(
        router.route(guild_create(
            OTHER_GUILD,
            ALICE,
            &[role_json(MOD_ROLE, ADMINISTRATOR)]
        )),
        None
    );
    assert!(!alice_admin(&mut router, &[MOD_ROLE]));
}
