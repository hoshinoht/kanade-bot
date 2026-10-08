//! Gateway roster and guild-access events against the store-backed staff
//! gate: persisted roles and Administrator, and sessions ended on loss.

use kanade::{
    api::{
        auth::{
            audit::AuditEvent,
            roster::{on_guild_available, on_roster_update},
            wire,
        },
        avatars::AvatarCache,
    },
    bot::events::{AdminRoles, RosterUpdate},
    domain::members::MemberStore,
};
use twilight_model::id::Id;

use super::{ADMIN_ROLE, Harness, cookie, user};

const BOSSING: u64 = 10;

fn seen(user_id: u64, roles: &[u64], is_guild_admin: bool) -> RosterUpdate {
    RosterUpdate::Seen {
        user_id: user_id.to_string(),
        display_name: format!("member-{user_id}"),
        nickname: None,
        has_role: roles.contains(&BOSSING),
        roles: roles.iter().map(u64::to_string).collect(),
        is_guild_admin,
    }
}

fn admin_roles(roles: &[u64]) -> AdminRoles {
    AdminRoles {
        everyone: false,
        roles: roles.iter().map(u64::to_string).collect(),
    }
}

async fn apply(harness: &Harness, update: RosterUpdate) -> u64 {
    let auth = harness.site.auth.clone().unwrap();
    on_roster_update(&auth, None, &*harness.store, None, &update)
        .await
        .unwrap()
}

async fn guild(harness: &Harness, owner: u64, admin: &[u64]) -> u64 {
    let auth = harness.site.auth.clone().unwrap();
    on_guild_available(
        &auth,
        &harness.access,
        &*harness.store,
        Id::new(owner),
        &admin_roles(admin),
    )
    .await
    .unwrap()
}

async fn login(harness: &Harness, id: u64) -> (&'static str, String) {
    let reply = harness.discord_login(user(id, "Member"), "%2F").await;
    cookie(&reply.cookie(wire::SESSION_COOKIE).expect("staff signs in"))
}

async fn status(harness: &Harness, session: &(&'static str, String)) -> u16 {
    harness
        .get("/api/admin/session", &[(session.0, &session.1)])
        .await
        .status
}

#[tokio::test]
async fn member_updates_store_roles_and_administrator_and_end_sessions_on_loss() {
    let harness = Harness::store_gated().await;
    assert_eq!(
        apply(&harness, seen(111, &[BOSSING, ADMIN_ROLE], false)).await,
        0
    );
    let row = harness.store.load_member("111").await.unwrap().unwrap();
    assert_eq!(row.roles, ["10", "20"]);
    let session = login(&harness, 111).await;

    // Admin role swapped for Administrator: still staff.
    assert_eq!(apply(&harness, seen(111, &[BOSSING], true)).await, 0);
    let row = harness.store.load_member("111").await.unwrap().unwrap();
    assert_eq!(
        (row.roles.as_slice(), row.is_guild_admin),
        (&["10".to_owned()][..], true)
    );
    assert_eq!(status(&harness, &session).await, 200);

    assert_eq!(apply(&harness, seen(111, &[BOSSING], false)).await, 1);
    assert_eq!(status(&harness, &session).await, 401, "no longer staff");
    assert!(harness.audit.events().contains(&AuditEvent::SessionEnded {
        actor: "discord:111".into(),
        reason: "not_staff",
    }));
}

#[tokio::test]
async fn guild_access_revokes_lost_administrator_and_a_replaced_owner() {
    let harness = Harness::store_gated().await;
    assert_eq!(guild(&harness, 333, &[30, 31]).await, 0);
    assert_eq!(apply(&harness, seen(222, &[30], true)).await, 0);
    assert_eq!(apply(&harness, seen(333, &[], false)).await, 0);
    assert_eq!(apply(&harness, seen(444, &[31], true)).await, 0);
    let sessions = [
        login(&harness, 222).await,
        login(&harness, 333).await,
        login(&harness, 444).await,
    ];

    // Role 30 lost Administrator and the guild changed hands.
    assert_eq!(guild(&harness, 444, &[31]).await, 2);
    let revoked = harness.store.load_member("222").await.unwrap().unwrap();
    assert!(!revoked.is_guild_admin);
    assert_eq!(revoked.roles, ["30"], "roles are the member's, kept");
    assert!(
        harness
            .store
            .load_member("444")
            .await
            .unwrap()
            .unwrap()
            .is_guild_admin
    );
    assert_eq!(status(&harness, &sessions[0]).await, 401);
    assert_eq!(status(&harness, &sessions[1]).await, 401, "former owner");
    assert_eq!(status(&harness, &sessions[2]).await, 200);

    // Grants wait for the member's own update: a stored row may be stale.
    assert_eq!(guild(&harness, 444, &[30, 31]).await, 0);
    let row = harness.store.load_member("222").await.unwrap().unwrap();
    assert!(!row.is_guild_admin);
}

/// A member who leaves loses their cached portrait; others keep theirs, and
/// a member who is only updated keeps it too.
#[tokio::test]
async fn leaving_purges_the_members_cached_portraits() {
    let harness = Harness::store_gated().await;
    let dir = std::env::temp_dir().join(format!("kanade-avatars-{}", uuid::Uuid::new_v4()));
    let avatars = AvatarCache::new(Some(dir.clone()), None);
    let hash = "0123456789abcdef0123456789abcdef";
    for name in [
        format!("111-{hash}.png"),
        format!("111-a_{hash}.png"),
        format!("1110-{hash}.png"),
        format!("222-{hash}.webp"),
    ] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let auth = harness.site.auth.clone().unwrap();
    on_roster_update(
        &auth,
        None,
        &*harness.store,
        Some(&avatars),
        &seen(222, &[BOSSING], false),
    )
    .await
    .unwrap();
    on_roster_update(
        &auth,
        None,
        &*harness.store,
        Some(&avatars),
        &RosterUpdate::Left {
            user_id: "111".into(),
        },
    )
    .await
    .unwrap();
    let mut left: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(
        left,
        [format!("1110-{hash}.png"), format!("222-{hash}.webp")]
    );
    std::fs::remove_dir_all(dir).unwrap();
}
