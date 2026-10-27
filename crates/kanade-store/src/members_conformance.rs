//! Member storage every store must keep: round trips, replace-in-place with
//! the ping level and aliases, alias uniqueness and validity, the tease
//! opt-in and running-joke note.

use super::fixtures::{refused, refused_for};
use kanade_domain::members::{
    GatewayMember, Member, MemberProfile, MemberStore, PingLevel, PortalEdit,
};

pub async fn run_suite<S: MemberStore>(make: impl AsyncFn() -> S) {
    members_round_trip_and_replace(make().await).await;
    aliases_are_unique_and_one_word(make().await).await;
    gateway_and_portal_writes_own_their_fields(make().await).await;
    interleaved_gateway_and_portal_writes_lose_nothing(make().await).await;
    portal_alias_removal_releases_the_alias(make().await).await;
    tease_fields_default_off_and_round_trip(make().await).await;
}

pub fn gateway(user_id: &str, name: &str, roles: &[&str]) -> GatewayMember {
    GatewayMember {
        user_id: user_id.into(),
        display_name: Some(name.into()),
        nickname: None,
        has_role: true,
        is_bot: false,
        roles: roles.iter().map(|role| (*role).to_owned()).collect(),
        is_guild_admin: false,
    }
}

async fn gateway_and_portal_writes_own_their_fields<S: MemberStore>(store: S) {
    assert_eq!(
        store
            .apply_portal("200", PortalEdit::default())
            .await
            .expect("portal"),
        None,
        "no row, nothing written"
    );
    store
        .apply_gateway(gateway("200", "Alice", &["20"]))
        .await
        .expect("gateway inserts");
    let edited = store
        .apply_portal(
            "200",
            PortalEdit {
                ping_level: Some(PingLevel::Off),
                reply_style: Some(Some("terse".into())),
                add_alias: Some("ali".into()),
                remove_alias: None,
                ..PortalEdit::default()
            },
        )
        .await
        .expect("portal")
        .expect("row");
    assert_eq!(edited.aliases, ["ali"]);
    assert_eq!(edited.roles, ["20"], "portal edits keep gateway fields");

    store
        .apply_gateway(gateway("200", "Alice B", &[]))
        .await
        .expect("gateway");
    let row = store.load_member("200").await.expect("load").expect("row");
    assert_eq!(row.member.display_name.as_deref(), Some("Alice B"));
    assert!(row.roles.is_empty());
    assert_eq!(
        (
            row.member.ping_level,
            row.reply_style.as_deref(),
            row.aliases.clone()
        ),
        (PingLevel::Off, Some("terse"), vec!["ali".to_owned()]),
        "gateway writes keep portal fields"
    );

    assert!(store.member_departed("200").await.expect("left"));
    assert!(store.clear_guild_admin("200").await.expect("admin"));
    let row = store.load_member("200").await.expect("load").expect("row");
    assert!(!row.member.has_role && row.roles.is_empty() && !row.is_guild_admin);
    assert_eq!(row.aliases, ["ali"]);
    assert!(!store.member_departed("999").await.expect("left"));

    store
        .apply_gateway(gateway("100", "Bob", &[]))
        .await
        .expect("gateway");
    let taken = store
        .apply_portal(
            "100",
            PortalEdit {
                ping_level: Some(PingLevel::All),
                add_alias: Some("ali".into()),
                ..PortalEdit::default()
            },
        )
        .await;
    refused_for("alias held by Alice", taken);
    let bob = store.load_member("100").await.expect("load").expect("row");
    assert_eq!(
        bob.member.ping_level,
        PingLevel::Essential,
        "nothing written"
    );
}

/// The race A3's load-then-put lost: gateway updates and portal edits to the
/// same member, interleaved at every await, keep both sides' fields.
async fn interleaved_gateway_and_portal_writes_lose_nothing<S: MemberStore>(store: S) {
    store
        .apply_gateway(gateway("300", "Cara 0", &[]))
        .await
        .expect("gateway");
    let rounds = 12;
    let gateway_side = async {
        for n in 1..=rounds {
            store
                .apply_gateway(gateway("300", &format!("Cara {n}"), &["20"]))
                .await
                .expect("gateway");
        }
    };
    let portal_side = async {
        for n in 0..rounds {
            store
                .apply_portal(
                    "300",
                    PortalEdit {
                        add_alias: Some(format!("c{n}")),
                        ping_level: Some(PingLevel::All),
                        ..PortalEdit::default()
                    },
                )
                .await
                .expect("portal");
        }
    };
    tokio::join!(gateway_side, portal_side);
    let row = store.load_member("300").await.expect("load").expect("row");
    assert_eq!(row.aliases.len(), rounds, "{:?}", row.aliases);
    assert_eq!(row.member.ping_level, PingLevel::All);
    assert_eq!(row.member.display_name.as_deref(), Some("Cara 12"));
    assert_eq!(row.roles, ["20"]);
}

fn remove(alias: &str) -> PortalEdit {
    PortalEdit {
        remove_alias: Some(alias.into()),
        ..PortalEdit::default()
    }
}

/// Removal keeps the other aliases in order and frees the alias for anyone;
/// a missing alias is a no-op and a missing row is `None`.
async fn portal_alias_removal_releases_the_alias<S: MemberStore>(store: S) {
    assert_eq!(
        store
            .apply_portal("200", remove("ali"))
            .await
            .expect("no row"),
        None
    );
    let mut alice = profile("200", "Alice");
    // `v4.name` is a stored v4 alias the portal input rule would refuse.
    alice.aliases = vec!["ali".into(), "v4.name".into(), "al".into()];
    alice.roles = vec!["20".into()];
    alice.member.ping_level = PingLevel::Off;
    store.put_member(alice.clone()).await.expect("put");
    store.put_member(profile("100", "Bob")).await.expect("put");

    for alias in ["v4.name", "ali"] {
        let edited = store
            .apply_portal("200", remove(alias))
            .await
            .expect("remove")
            .expect("row");
        assert!(!edited.aliases.iter().any(|held| held == alias), "{alias}");
    }
    let row = store.load_member("200").await.expect("load").expect("row");
    assert_eq!(row.aliases, ["al"], "the rest keep their order");
    assert_eq!(
        (row.roles.clone(), row.member.ping_level),
        (alice.roles.clone(), PingLevel::Off),
        "other fields untouched"
    );

    let unchanged = store
        .apply_portal("200", remove("ali"))
        .await
        .expect("idempotent")
        .expect("row");
    assert_eq!(unchanged, row, "an alias not held is a no-op");
    // Bob's alias is not Alice's to remove.
    let bob = store
        .apply_portal(
            "100",
            PortalEdit {
                add_alias: Some("ali".into()),
                ..PortalEdit::default()
            },
        )
        .await
        .expect("a released alias is free again")
        .expect("row");
    assert_eq!(bob.aliases, ["ali"]);
    store
        .apply_portal("200", remove("ali"))
        .await
        .expect("remove")
        .expect("row");
    let bob = store.load_member("100").await.expect("load").expect("row");
    assert_eq!(bob.aliases, ["ali"]);
    // The uniqueness index followed the removal: Alice cannot take it back.
    let taken = store
        .apply_portal(
            "200",
            PortalEdit {
                add_alias: Some("ali".into()),
                ..PortalEdit::default()
            },
        )
        .await;
    refused(taken);
    let bob = store
        .apply_portal(
            "100",
            PortalEdit {
                add_alias: Some("v4.name".into()),
                ..PortalEdit::default()
            },
        )
        .await
        .expect("the removed v4 alias is free too")
        .expect("row");
    assert_eq!(bob.aliases, ["ali", "v4.name"]);
}

pub fn profile(user_id: &str, name: &str) -> MemberProfile {
    MemberProfile {
        member: Member {
            user_id: user_id.into(),
            display_name: Some(name.into()),
            nickname: None,
            has_role: true,
            is_bot: false,
            ping_level: PingLevel::Essential,
        },
        aliases: Vec::new(),
        reply_style: None,
        roles: Vec::new(),
        is_guild_admin: false,
        tease_ok: false,
        running_joke: None,
    }
}

async fn members_round_trip_and_replace<S: MemberStore>(store: S) {
    assert_eq!(store.list_members().await.expect("list"), Vec::new());
    let mut alice = profile("200", "Alice");
    alice.aliases = vec!["ali".into(), "アリス".into()];
    alice.reply_style = Some("terse".into());
    alice.roles = vec!["20".into(), "10".into()];
    alice.is_guild_admin = true;
    alice.member.nickname = Some("Al".into());
    alice.member.ping_level = PingLevel::Off;
    store.put_member(alice.clone()).await.expect("put");
    store.put_member(profile("100", "Bob")).await.expect("put");

    let listed = store.list_members().await.expect("list");
    assert_eq!(
        listed
            .iter()
            .map(|p| p.member.user_id.as_str())
            .collect::<Vec<_>>(),
        ["100", "200"],
        "by user id"
    );
    assert_eq!(
        store.load_member("200").await.expect("load"),
        Some(alice.clone())
    );
    assert_eq!(store.load_member("300").await.expect("load"), None);

    let mut changed = alice.clone();
    changed.aliases = vec!["al".into()];
    changed.roles.clear();
    changed.is_guild_admin = false;
    store.put_member(changed.clone()).await.expect("replace");
    assert_eq!(store.load_member("200").await.expect("load"), Some(changed));
    // The dropped alias is free again.
    let mut bob = profile("100", "Bob");
    bob.aliases = vec!["ali".into()];
    store.put_member(bob).await.expect("reuse a released alias");
}

async fn aliases_are_unique_and_one_word<S: MemberStore>(store: S) {
    let mut alice = profile("200", "Alice");
    alice.aliases = vec!["ali".into()];
    store.put_member(alice).await.expect("put");
    for (case, aliases) in [
        ("held by another member", vec!["ali"]),
        ("repeated", vec!["bo", "bo"]),
        ("uppercase", vec!["Bob"]),
        ("two words", vec!["bo b"]),
        ("empty", vec![""]),
    ] {
        let mut bob = profile("100", "Bob");
        bob.aliases = aliases.into_iter().map(str::to_owned).collect();
        refused_for(case, store.put_member(bob).await);
        assert_eq!(
            store.load_member("100").await.expect("load"),
            None,
            "{case}: nothing written"
        );
    }
}

/// The tease opt-in starts off with no note; a put, a portal edit and a
/// gateway write each keep the other side's fields, and the note is refused
/// past 80 characters with nothing written.
async fn tease_fields_default_off_and_round_trip<S: MemberStore>(store: S) {
    store
        .apply_gateway(gateway("200", "Alice", &["20"]))
        .await
        .expect("gateway inserts");
    let row = store.load_member("200").await.expect("load").expect("row");
    assert_eq!(
        (row.tease_ok, row.running_joke),
        (false, None),
        "default off"
    );

    let mut alice = profile("200", "Alice");
    alice.aliases = vec!["ali".into()];
    alice.tease_ok = true;
    alice.running_joke = Some("Still owes the guild a Lucid clear".into());
    store.put_member(alice.clone()).await.expect("put");
    assert_eq!(
        store.load_member("200").await.expect("load"),
        Some(alice.clone()),
        "round trip"
    );
    alice.member.nickname = Some("Al".into());
    store.put_member(alice.clone()).await.expect("upsert");
    let row = store.load_member("200").await.expect("load").expect("row");
    assert_eq!(row, alice, "an upsert keeps the aliases and tease fields");

    let edited = store
        .apply_portal(
            "200",
            PortalEdit {
                tease_ok: Some(false),
                ..PortalEdit::default()
            },
        )
        .await
        .expect("portal")
        .expect("row");
    assert!(!edited.tease_ok);
    assert_eq!(
        (edited.running_joke.as_deref(), edited.aliases.clone()),
        (alice.running_joke.as_deref(), vec!["ali".to_owned()]),
        "an opt-out keeps the note and aliases"
    );
    store
        .apply_gateway(gateway("200", "Alice B", &[]))
        .await
        .expect("gateway");
    let row = store.load_member("200").await.expect("load").expect("row");
    assert_eq!(
        row.running_joke, alice.running_joke,
        "gateway writes keep the note"
    );

    let long = PortalEdit {
        running_joke: Some(Some("x".repeat(81))),
        ..PortalEdit::default()
    };
    refused_for(
        "an 81-character note",
        store.apply_portal("200", long).await,
    );
    let cleared = store
        .apply_portal(
            "200",
            PortalEdit {
                running_joke: Some(None),
                ..PortalEdit::default()
            },
        )
        .await
        .expect("portal")
        .expect("row");
    assert_eq!(cleared.running_joke, None, "Some(None) clears the note");
}
