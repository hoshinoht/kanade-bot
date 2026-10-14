//! Roster characters every store must keep: IGNs unique per world,
//! case-insensitively (retired characters included); one active main per
//! member with active characters, a new main taking the crown in the same
//! write and the main kept while others are active; edits and deletes behind
//! a version precondition; every effective write recorded in History
//! (section `characters`) with it; keyed writes replayed in the same
//! transaction; rows kept when their member leaves.

use chrono::{DateTime, TimeZone, Timelike, Utc};

use super::fixtures::refused;
use crate::domain::characters::{
    CHARACTERS_SECTION, Character, CharacterChange, CharacterEdit, CharacterRecord, CharacterRole,
    CharacterStore, CharacterWrite,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::members::MemberStore;
use crate::domain::settings::{SettingsChange, SettingsChangeQuery, SettingsStore};
use crate::infrastructure::store::members_conformance::gateway;
use crate::infrastructure::store::replays::{KeyedWrite, ReplayScope, ReplayStore, StoredReplay};

pub async fn run_suite<S: CharacterStore + ReplayStore + SettingsStore + MemberStore>(
    make: impl AsyncFn() -> S,
) {
    characters_round_trip_and_list_by_member(make().await).await;
    igns_are_unique_per_world_case_insensitively(make().await).await;
    a_new_main_takes_the_crown(make().await).await;
    the_main_stays_while_others_are_active(make().await).await;
    edits_need_the_current_version(make().await).await;
    retire_and_delete(make().await).await;
    a_departed_member_keeps_their_characters(make().await).await;
    keyed_writes_replay_in_the_same_write(make().await).await;
    malformed_rows_are_refused(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 11, hour, 0, 0).unwrap()
}

fn admin() -> Actor {
    Actor::admin("token")
}

fn record(hour: u32) -> CharacterRecord {
    CharacterRecord {
        at: at(hour),
        actor: admin(),
        surface: Surface::AdminPortal,
    }
}

fn character(id: &str, member: &str, ign: &str, role: CharacterRole, hour: u32) -> Character {
    Character {
        id: id.into(),
        member_id: member.into(),
        ign: ign.into(),
        world: "msea".into(),
        class: Some("Bishop".into()),
        level: Some(250),
        role,
        retired_at: None,
        created_at: at(hour),
        updated_at: at(hour),
        updated_by: admin(),
        version: 1,
    }
}

fn main(id: &str, member: &str, ign: &str, hour: u32) -> Character {
    character(id, member, ign, CharacterRole::Main, hour)
}

fn alt(id: &str, member: &str, ign: &str, hour: u32) -> Character {
    character(id, member, ign, CharacterRole::Alt, hour)
}

/// Created at its own `created_at` (every instant here is on one day).
async fn create<S: CharacterStore>(store: &S, row: &Character) -> CharacterWrite {
    store
        .apply_character(
            CharacterChange::Create(row.clone()),
            record(row.created_at.hour()),
        )
        .await
        .expect("create")
}

/// `edit` with an explicit `role` where it matters (no role inference).
async fn update<S: CharacterStore>(
    store: &S,
    row: &Character,
    edit: CharacterEdit,
    hour: u32,
) -> CharacterWrite {
    let mut after = row.clone();
    if let Some(ign) = edit.ign {
        after.ign = ign;
    }
    if let Some(level) = edit.level {
        after.level = level;
    }
    if let Some(role) = edit.role {
        after.role = role;
    }
    match edit.retired {
        Some(true) if after.retired_at.is_none() => after.retired_at = Some(at(hour)),
        Some(false) => after.retired_at = None,
        _ => {}
    }
    let after = if after.same_content(row) {
        after
    } else {
        after.next_version(at(hour), &admin())
    };
    store
        .apply_character(
            CharacterChange::Update {
                version: row.version,
                after,
            },
            record(hour),
        )
        .await
        .expect("update")
}

fn retire() -> CharacterEdit {
    CharacterEdit {
        retired: Some(true),
        ..CharacterEdit::default()
    }
}

fn role(role: CharacterRole) -> CharacterEdit {
    CharacterEdit {
        role: Some(role),
        ..CharacterEdit::default()
    }
}

async fn stored<S: CharacterStore>(store: &S, id: &str) -> Character {
    store.character(id).await.unwrap().expect("stored")
}

async fn history<S: SettingsStore>(store: &S) -> Vec<SettingsChange> {
    store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .expect("history")
}

async fn characters_round_trip_and_list_by_member<S: CharacterStore + SettingsStore>(store: S) {
    let first = main("c-1", "1001", "Lilac", 1);
    let second = alt("c-2", "1001", "Wisteria", 2);
    let other = main("c-3", "1002", "Heather", 3);
    for row in [&first, &second, &other] {
        assert_eq!(
            create(&store, row).await,
            CharacterWrite::Created(row.clone())
        );
    }
    assert_eq!(stored(&store, "c-2").await, second);
    assert_eq!(store.character("c-9").await.unwrap(), None);
    assert_eq!(
        store.member_characters("1001").await.unwrap(),
        [first, second],
        "one member's rows, oldest first"
    );
    assert!(store.member_characters("1003").await.unwrap().is_empty());

    let saves = history(&store).await;
    assert_eq!(saves.len(), 3, "one History row per create");
    let save = &saves[0];
    assert_eq!(
        (
            save.section.as_str(),
            save.revision,
            &save.actor,
            save.surface
        ),
        (CHARACTERS_SECTION, 0, &admin(), Surface::AdminPortal)
    );
    let row = save.values.get("character.c-3").expect("the newest create");
    assert_eq!(row.from, "", "a create has no before side");
    assert!(row.to.contains(r#""ign":"Heather""#), "{row:?}");
    assert!(row.to.contains(r#""member":"1002""#), "{row:?}");
}

async fn igns_are_unique_per_world_case_insensitively<S: CharacterStore + SettingsStore>(store: S) {
    let first = main("c-1", "1001", "Wisteria", 1);
    create(&store, &first).await;
    let shouted = main("c-2", "1002", "WISTERIA", 2);
    assert_eq!(create(&store, &shouted).await, CharacterWrite::IgnTaken);
    assert_eq!(
        store.character("c-2").await.unwrap(),
        None,
        "nothing written"
    );
    let elsewhere = Character {
        world: "gms".into(),
        ..shouted.clone()
    };
    assert!(
        create(&store, &elsewhere).await.applied(),
        "another world is another namespace"
    );

    let second = main("c-3", "1003", "Heather", 3);
    create(&store, &second).await;
    let rename = CharacterEdit {
        ign: Some("wisteria".into()),
        ..CharacterEdit::default()
    };
    assert_eq!(
        update(&store, &second, rename, 4).await,
        CharacterWrite::IgnTaken,
        "a rename meets the same rule"
    );
    // A retired character keeps its IGN.
    let CharacterWrite::Updated(retired) = update(&store, &first, retire(), 5).await else {
        panic!("retired");
    };
    assert!(retired.retired_at.is_some());
    let again = main("c-4", "1004", "wisTeria", 6);
    assert_eq!(create(&store, &again).await, CharacterWrite::IgnTaken);
    // The row's own IGN in another case is a rename, not a clash.
    let recased = CharacterEdit {
        ign: Some("HEATHER".into()),
        ..CharacterEdit::default()
    };
    let CharacterWrite::Updated(recased) = update(&store, &second, recased, 7).await else {
        panic!("recased");
    };
    assert_eq!(recased.ign, "HEATHER");
    assert_eq!(history(&store).await.len(), 5, "refusals record nothing");
}

async fn a_new_main_takes_the_crown<S: CharacterStore + SettingsStore>(store: S) {
    let first = alt("c-1", "1001", "Lilac", 1);
    assert_eq!(
        create(&store, &first).await,
        CharacterWrite::MainRequired,
        "a member's first character is their main"
    );
    let first = main("c-1", "1001", "Lilac", 1);
    create(&store, &first).await;
    let second = main("c-2", "1001", "Wisteria", 2);
    assert!(create(&store, &second).await.applied());
    let demoted = stored(&store, "c-1").await;
    assert_eq!(
        (demoted.role, demoted.version, demoted.updated_at),
        (CharacterRole::Alt, 2, at(2)),
        "the old main became an alt in the same write"
    );
    let saves = history(&store).await;
    assert_eq!(saves.len(), 2);
    assert_eq!(
        saves[0].values.keys().collect::<Vec<_>>(),
        ["character.c-1", "character.c-2"],
        "one History row lists both"
    );
    assert!(
        saves[0].values["character.c-1"]
            .to
            .contains(r#""role":"alt""#)
    );
    // Another member's main is no clash.
    assert!(
        create(&store, &main("c-3", "1002", "Heather", 3))
            .await
            .applied()
    );

    // Promoting an alt moves the crown back.
    let CharacterWrite::Updated(promoted) =
        update(&store, &demoted, role(CharacterRole::Main), 4).await
    else {
        panic!("promoted");
    };
    assert_eq!(promoted.role, CharacterRole::Main);
    assert_eq!(stored(&store, "c-2").await.role, CharacterRole::Alt);
    let mains: Vec<String> = store
        .member_characters("1001")
        .await
        .unwrap()
        .into_iter()
        .filter(Character::is_active_main)
        .map(|row| row.id)
        .collect();
    assert_eq!(mains, ["c-1"], "exactly one main");
}

async fn the_main_stays_while_others_are_active<S: CharacterStore + SettingsStore>(store: S) {
    let first = main("c-1", "1001", "Lilac", 1);
    let second = alt("c-2", "1001", "Wisteria", 2);
    create(&store, &first).await;
    create(&store, &second).await;
    assert_eq!(
        update(&store, &first, retire(), 3).await,
        CharacterWrite::MainRequired
    );
    assert_eq!(
        update(&store, &first, role(CharacterRole::Alt), 3).await,
        CharacterWrite::MainRequired
    );
    assert_eq!(
        store
            .apply_character(
                CharacterChange::Delete {
                    id: "c-1".into(),
                    version: 1
                },
                record(3)
            )
            .await
            .unwrap(),
        CharacterWrite::MainRequired
    );
    assert_eq!(history(&store).await.len(), 2, "refusals record nothing");

    // Once the alt is retired the main is alone and may go.
    let CharacterWrite::Updated(_) = update(&store, &second, retire(), 4).await else {
        panic!("alt retired");
    };
    let CharacterWrite::Updated(retired_main) = update(&store, &first, retire(), 5).await else {
        panic!("main retired");
    };
    // With no active character, the next one must be the main again.
    assert_eq!(
        create(&store, &alt("c-3", "1001", "Heather", 6)).await,
        CharacterWrite::MainRequired
    );
    create(&store, &main("c-3", "1001", "Heather", 6)).await;
    // A restored old main asking for the crown takes it.
    let restore = CharacterEdit {
        retired: Some(false),
        ..CharacterEdit::default()
    };
    assert!(update(&store, &retired_main, restore, 7).await.applied());
    assert_eq!(stored(&store, "c-3").await.role, CharacterRole::Alt);
}

async fn edits_need_the_current_version<S: CharacterStore + SettingsStore>(store: S) {
    let row = main("c-1", "1001", "Wisteria", 1);
    create(&store, &row).await;
    let level = |level| CharacterEdit {
        level: Some(Some(level)),
        ..CharacterEdit::default()
    };
    let CharacterWrite::Updated(edited) = update(&store, &row, level(260), 2).await else {
        panic!("edited");
    };
    assert_eq!(
        (
            edited.version,
            edited.level,
            edited.updated_at,
            edited.created_at
        ),
        (2, Some(260), at(2), at(1))
    );
    assert_eq!(stored(&store, "c-1").await, edited);
    let saves = history(&store).await;
    let diff = &saves[0].values["character.c-1"];
    assert!(diff.from.contains(r#""level":250"#), "{diff:?}");
    assert!(diff.to.contains(r#""level":260"#), "{diff:?}");

    // An edit from the old version is stale and answers the row as it is.
    assert_eq!(
        update(&store, &row, level(270), 3).await,
        CharacterWrite::Stale(edited.clone())
    );
    // A no-op writes and records nothing, even at the right version.
    assert_eq!(
        update(&store, &edited, level(260), 4).await,
        CharacterWrite::Updated(edited.clone())
    );
    assert_eq!(history(&store).await.len(), 2, "create and the one edit");
    assert_eq!(
        update(&store, &main("c-9", "1001", "Nobody", 1), level(1), 5).await,
        CharacterWrite::Missing
    );
    // An update may not move a row to another member or skip a version.
    for after in [
        Character {
            member_id: "1002".into(),
            version: 3,
            ..edited.clone()
        },
        Character {
            level: Some(1),
            version: 4,
            ..edited.clone()
        },
    ] {
        refused(
            store
                .apply_character(CharacterChange::Update { version: 2, after }, record(6))
                .await,
        );
    }
    assert_eq!(stored(&store, "c-1").await, edited);
}

async fn retire_and_delete<S: CharacterStore + SettingsStore>(store: S) {
    create(&store, &main("c-0", "1001", "Lilac", 1)).await;
    let row = alt("c-1", "1001", "Wisteria", 1);
    create(&store, &row).await;
    let CharacterWrite::Updated(retired) = update(&store, &row, retire(), 2).await else {
        panic!("retired");
    };
    assert_eq!(retired.retired_at, Some(at(2)));
    assert_eq!(
        store.member_characters("1001").await.unwrap().len(),
        2,
        "a retired character is still listed"
    );

    let delete = |version| CharacterChange::Delete {
        id: "c-1".into(),
        version,
    };
    assert_eq!(
        store.apply_character(delete(1), record(3)).await.unwrap(),
        CharacterWrite::Stale(retired.clone())
    );
    assert_eq!(
        store.apply_character(delete(2), record(3)).await.unwrap(),
        CharacterWrite::Deleted(retired)
    );
    assert_eq!(store.character("c-1").await.unwrap(), None);
    assert_eq!(
        store.apply_character(delete(2), record(4)).await.unwrap(),
        CharacterWrite::Missing
    );
    let saves = history(&store).await;
    assert_eq!(saves.len(), 4, "two creates, retire, delete");
    let deleted = &saves[0].values["character.c-1"];
    assert!(deleted.from.contains(r#""retired":true"#), "{deleted:?}");
    assert_eq!(deleted.to, "", "a delete has no after side");
    // The IGN is free again once deleted.
    assert!(
        create(&store, &main("c-2", "1002", "wisteria", 5))
            .await
            .applied()
    );
    // The main alone may be deleted.
    let lone = stored(&store, "c-2").await;
    assert!(
        store
            .apply_character(
                CharacterChange::Delete {
                    id: lone.id,
                    version: lone.version
                },
                record(6)
            )
            .await
            .unwrap()
            .applied()
    );
}

async fn a_departed_member_keeps_their_characters<S: CharacterStore + MemberStore>(store: S) {
    store
        .apply_gateway(gateway("1001", "Alice", &["10"]))
        .await
        .unwrap();
    let row = main("c-1", "1001", "Wisteria", 1);
    create(&store, &row).await;
    assert!(store.member_departed("1001").await.unwrap());
    assert_eq!(store.member_characters("1001").await.unwrap(), [row]);
}

async fn keyed_writes_replay_in_the_same_write<S: CharacterStore + ReplayStore + SettingsStore>(
    store: S,
) {
    let replay = |key: &str, request: char, status| {
        StoredReplay::new(
            ReplayScope::Config,
            "admin:token".into(),
            key.into(),
            std::iter::repeat_n(request, 64).collect(),
            status,
            r#"{"id":"c-1"}"#.into(),
            at(1),
        )
    };
    let row = main("c-1", "1001", "Wisteria", 1);
    let keyed = |row: &Character, key: &str, request| {
        store.apply_character_replayed(
            CharacterChange::Create(row.clone()),
            record(1),
            replay(key, request, 201),
        )
    };
    assert_eq!(
        keyed(&row, "k", 'a').await.unwrap(),
        KeyedWrite::Applied(CharacterWrite::Created(row.clone()))
    );
    let found = store
        .replay(ReplayScope::Config, "admin:token", "k", at(2))
        .await
        .unwrap()
        .expect("the answer is stored with the write");
    assert_eq!(found.status, 201);
    assert_eq!(
        keyed(&row, "k", 'a').await.unwrap(),
        KeyedWrite::Replayed(found),
        "the same request replays"
    );
    let other = main("c-2", "1002", "Heather", 1);
    assert_eq!(keyed(&other, "k", 'b').await.unwrap(), KeyedWrite::KeyTaken);
    assert_eq!(store.character("c-2").await.unwrap(), None);
    assert_eq!(history(&store).await.len(), 1, "replays record nothing");

    // A refusal stores no answer: the key applies again later.
    let clash = main("c-3", "1002", "WISTERIA", 1);
    assert_eq!(
        keyed(&clash, "k-2", 'c').await.unwrap(),
        KeyedWrite::Applied(CharacterWrite::IgnTaken)
    );
    assert_eq!(
        store
            .replay(ReplayScope::Config, "admin:token", "k-2", at(2))
            .await
            .unwrap(),
        None
    );
    let renamed = main("c-3", "1002", "Bluebell", 1);
    assert_eq!(
        keyed(&renamed, "k-2", 'c').await.unwrap(),
        KeyedWrite::Applied(CharacterWrite::Created(renamed.clone()))
    );
}

async fn malformed_rows_are_refused<S: CharacterStore + SettingsStore>(store: S) {
    let good = main("c-1", "1001", "Wisteria", 1);
    for bad in [
        Character {
            level: Some(301),
            ..good.clone()
        },
        Character {
            class: Some(String::new()),
            ..good.clone()
        },
        Character {
            version: 2,
            ..good.clone()
        },
        Character {
            world: String::new(),
            ..good.clone()
        },
    ] {
        refused(
            store
                .apply_character(CharacterChange::Create(bad), record(1))
                .await,
        );
    }
    create(&store, &good).await;
    refused(
        store
            .apply_character(CharacterChange::Create(good), record(2))
            .await,
    );
    assert_eq!(history(&store).await.len(), 1);
}
