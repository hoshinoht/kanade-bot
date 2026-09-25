//! Member storage every store must keep: round trips, replace-in-place with
//! the ping level and aliases, alias uniqueness and validity.

use crate::domain::members::{Member, MemberProfile, MemberStore, PingLevel};
use crate::domain::scheduler::StoreError;

pub async fn run_suite<S: MemberStore>(make: impl AsyncFn() -> S) {
    members_round_trip_and_replace(make().await).await;
    aliases_are_unique_and_one_word(make().await).await;
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
        assert!(
            matches!(store.put_member(bob).await, Err(StoreError::Constraint(_))),
            "{case}"
        );
        assert_eq!(
            store.load_member("100").await.expect("load"),
            None,
            "{case}: nothing written"
        );
    }
}
