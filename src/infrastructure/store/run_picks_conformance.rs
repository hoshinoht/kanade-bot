//! Run picks every store must keep: one stored pick per `(run, member)`
//! behind a slot version (0 with none); a set names an active character of
//! that member; a clear deletes the row; a no-op or refusal writes and
//! records nothing; every effective write is recorded in History (section
//! `characters`, key `pick.<run>.<member>`) with it; keyed writes replay in
//! the same transaction; deleting a character drops its picks.

use chrono::{DateTime, TimeZone, Utc};

use crate::domain::characters::{
    CHARACTERS_SECTION, Character, CharacterChange, CharacterEdit, CharacterRecord, CharacterRole,
    CharacterStore, PickChange, PickWrite, RunPick,
};
use crate::domain::history::{Actor, Surface};
use crate::domain::scheduler::StoreError;
use crate::domain::settings::{SettingsChange, SettingsChangeQuery, SettingsStore};
use crate::infrastructure::store::replays::{KeyedWrite, ReplayScope, ReplayStore, StoredReplay};

pub async fn run_suite<S: CharacterStore + ReplayStore + SettingsStore>(make: impl AsyncFn() -> S) {
    picks_are_set_changed_and_cleared_behind_a_version(make().await).await;
    picks_name_an_active_character_of_the_member(make().await).await;
    picks_are_read_by_run(make().await).await;
    deleting_a_character_drops_its_picks(make().await).await;
    keyed_picks_replay_in_the_same_write(make().await).await;
    malformed_picks_are_refused(make().await).await;
}

fn at(hour: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 12, hour, 0, 0).unwrap()
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
        class: Some("Night Lord".into()),
        level: Some(280),
        role,
        retired_at: None,
        created_at: at(hour),
        updated_at: at(hour),
        updated_by: admin(),
        version: 1,
    }
}

/// Alice (1001): Lilac (main) and Wisteria; Bob (1002): Heather (main).
async fn seeded<S: CharacterStore>(store: &S) {
    for row in [
        character("c-1", "1001", "Lilac", CharacterRole::Main, 1),
        character("c-2", "1001", "Wisteria", CharacterRole::Alt, 2),
        character("c-3", "1002", "Heather", CharacterRole::Main, 3),
    ] {
        assert!(
            store
                .apply_character(CharacterChange::Create(row), record(1))
                .await
                .unwrap()
                .applied()
        );
    }
}

fn set(run: &str, member: &str, character: &str, version: u32) -> PickChange {
    PickChange::Set {
        run_id: run.into(),
        member_id: member.into(),
        character_id: character.into(),
        version,
    }
}

fn clear(run: &str, member: &str, version: u32) -> PickChange {
    PickChange::Clear {
        run_id: run.into(),
        member_id: member.into(),
        version,
    }
}

fn pick(run: &str, member: &str, character: &str, hour: u32, version: u32) -> RunPick {
    RunPick {
        run_id: run.into(),
        member_id: member.into(),
        character_id: character.into(),
        picked_at: at(hour),
        picked_by: admin(),
        version,
    }
}

async fn picks<S: CharacterStore>(store: &S, runs: &[&str]) -> Vec<RunPick> {
    let runs: Vec<String> = runs.iter().map(|run| (*run).to_owned()).collect();
    store.run_picks(&runs).await.expect("picks")
}

/// The pick rows of History, newest first.
async fn history<S: SettingsStore>(store: &S) -> Vec<SettingsChange> {
    store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .expect("history")
        .into_iter()
        .filter(|change| change.values.keys().any(|key| key.starts_with("pick.")))
        .collect()
}

async fn picks_are_set_changed_and_cleared_behind_a_version<S: CharacterStore + SettingsStore>(
    store: S,
) {
    seeded(&store).await;
    let first = pick("r-1", "1001", "c-1", 4, 1);
    assert_eq!(
        store
            .apply_pick(set("r-1", "1001", "c-1", 0), record(4))
            .await
            .unwrap(),
        PickWrite::Picked(first.clone())
    );
    assert_eq!(picks(&store, &["r-1"]).await, std::slice::from_ref(&first));
    // The same character again is a no-op: no write, no record.
    assert_eq!(
        store
            .apply_pick(set("r-1", "1001", "c-1", 1), record(5))
            .await
            .unwrap(),
        PickWrite::Picked(first.clone())
    );
    // A write from version 0 now finds version 1.
    assert_eq!(
        store
            .apply_pick(set("r-1", "1001", "c-2", 0), record(5))
            .await
            .unwrap(),
        PickWrite::Stale(Some(first.clone()))
    );
    let second = pick("r-1", "1001", "c-2", 6, 2);
    assert_eq!(
        store
            .apply_pick(set("r-1", "1001", "c-2", 1), record(6))
            .await
            .unwrap(),
        PickWrite::Picked(second.clone())
    );
    assert_eq!(picks(&store, &["r-1"]).await, [second]);
    assert_eq!(
        store
            .apply_pick(clear("r-1", "1001", 2), record(7))
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert!(picks(&store, &["r-1"]).await.is_empty());
    // Clearing nothing is a no-op at version 0.
    assert_eq!(
        store
            .apply_pick(clear("r-1", "1001", 0), record(8))
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert_eq!(
        store
            .apply_pick(clear("r-1", "1001", 1), record(8))
            .await
            .unwrap(),
        PickWrite::Stale(None)
    );

    let saves = history(&store).await;
    assert_eq!(saves.len(), 3, "set, change, clear");
    let key = "pick.r-1.1001";
    let cleared = &saves[0];
    assert_eq!(cleared.section, CHARACTERS_SECTION);
    assert_eq!(cleared.at, at(7));
    assert_eq!(cleared.actor, admin());
    assert_eq!(cleared.revision, 0);
    assert!(cleared.values[key].from.contains(r#""ign":"Wisteria""#));
    assert_eq!(cleared.values[key].to, "");
    let changed = &saves[1].values[key];
    assert!(changed.from.contains(r#""character":"c-1""#), "{changed:?}");
    assert!(changed.to.contains(r#""character":"c-2""#), "{changed:?}");
    let created = &saves[2].values[key];
    assert_eq!(created.from, "");
    assert!(created.to.contains(r#""ign":"Lilac""#), "{created:?}");
    assert!(created.to.contains(r#""run":"r-1""#), "{created:?}");
}

async fn picks_name_an_active_character_of_the_member<S: CharacterStore + SettingsStore>(store: S) {
    seeded(&store).await;
    for (change, refused) in [
        (set("r-1", "1001", "c-9", 0), PickWrite::UnknownCharacter),
        (set("r-1", "1001", "c-3", 0), PickWrite::NotTheirs),
    ] {
        assert_eq!(store.apply_pick(change, record(4)).await.unwrap(), refused);
    }
    let wisteria = store.character("c-2").await.unwrap().unwrap();
    let retired = wisteria.edited(
        &CharacterEdit {
            retired: Some(true),
            ..CharacterEdit::default()
        },
        &[],
        at(4),
        &admin(),
    );
    assert!(
        store
            .apply_character(
                CharacterChange::Update {
                    version: 1,
                    after: retired
                },
                record(4)
            )
            .await
            .unwrap()
            .applied()
    );
    assert_eq!(
        store
            .apply_pick(set("r-1", "1001", "c-2", 0), record(5))
            .await
            .unwrap(),
        PickWrite::Retired
    );
    assert!(picks(&store, &["r-1"]).await.is_empty());
    assert!(history(&store).await.is_empty(), "refusals record nothing");
}

async fn picks_are_read_by_run<S: CharacterStore>(store: S) {
    seeded(&store).await;
    for change in [
        set("r-1", "1001", "c-1", 0),
        set("r-1", "1002", "c-3", 0),
        set("r-2", "1001", "c-2", 0),
        set("r-3", "1002", "c-3", 0),
    ] {
        assert!(store.apply_pick(change, record(4)).await.unwrap().applied());
    }
    let slots = |found: Vec<RunPick>| -> Vec<(String, String, String)> {
        found
            .into_iter()
            .map(|pick| (pick.run_id, pick.member_id, pick.character_id))
            .collect()
    };
    let owned = |run: &str, member: &str, character: &str| {
        (run.to_owned(), member.to_owned(), character.to_owned())
    };
    assert_eq!(
        slots(picks(&store, &["r-2", "r-1"]).await),
        [
            owned("r-1", "1001", "c-1"),
            owned("r-1", "1002", "c-3"),
            owned("r-2", "1001", "c-2"),
        ],
        "by run, then member"
    );
    assert!(picks(&store, &["r-9"]).await.is_empty());
    assert!(picks(&store, &[]).await.is_empty());
}

async fn deleting_a_character_drops_its_picks<S: CharacterStore + SettingsStore>(store: S) {
    seeded(&store).await;
    for change in [set("r-1", "1001", "c-2", 0), set("r-2", "1001", "c-1", 0)] {
        assert!(store.apply_pick(change, record(4)).await.unwrap().applied());
    }
    assert!(
        store
            .apply_character(
                CharacterChange::Delete {
                    id: "c-2".into(),
                    version: 1
                },
                record(5)
            )
            .await
            .unwrap()
            .applied()
    );
    assert_eq!(
        picks(&store, &["r-1", "r-2"]).await,
        [pick("r-2", "1001", "c-1", 4, 1)],
        "only the deleted character's pick goes"
    );
    // The slot is free again: a new pick starts at version 1.
    assert_eq!(
        store
            .apply_pick(set("r-1", "1001", "c-1", 0), record(6))
            .await
            .unwrap(),
        PickWrite::Picked(pick("r-1", "1001", "c-1", 6, 1))
    );
}

async fn keyed_picks_replay_in_the_same_write<S: CharacterStore + ReplayStore + SettingsStore>(
    store: S,
) {
    seeded(&store).await;
    let replay = |key: &str, request: char| {
        StoredReplay::new(
            ReplayScope::Config,
            "admin:token".into(),
            key.into(),
            std::iter::repeat_n(request, 64).collect(),
            200,
            r#"{"member_id":"1001"}"#.into(),
            at(4),
        )
    };
    let first = pick("r-1", "1001", "c-1", 4, 1);
    assert_eq!(
        store
            .apply_pick_replayed(set("r-1", "1001", "c-1", 0), record(4), replay("k", 'a'))
            .await
            .unwrap(),
        KeyedWrite::Applied(PickWrite::Picked(first.clone()))
    );
    let found = store
        .replay(ReplayScope::Config, "admin:token", "k", at(5))
        .await
        .unwrap()
        .expect("the answer is stored with the write");
    assert_eq!(found.status, 200);
    // The retry carries the version it was made from; it replays, not stale.
    assert_eq!(
        store
            .apply_pick_replayed(set("r-1", "1001", "c-1", 0), record(5), replay("k", 'a'))
            .await
            .unwrap(),
        KeyedWrite::Replayed(found)
    );
    assert_eq!(
        store
            .apply_pick_replayed(set("r-1", "1001", "c-2", 1), record(5), replay("k", 'b'))
            .await
            .unwrap(),
        KeyedWrite::KeyTaken
    );
    assert_eq!(picks(&store, &["r-1"]).await, [first]);
    assert_eq!(history(&store).await.len(), 1, "replays record nothing");

    // A refusal stores no answer: the key applies again later.
    assert_eq!(
        store
            .apply_pick_replayed(set("r-1", "1001", "c-3", 1), record(6), replay("k-2", 'c'))
            .await
            .unwrap(),
        KeyedWrite::Applied(PickWrite::NotTheirs)
    );
    assert_eq!(
        store
            .replay(ReplayScope::Config, "admin:token", "k-2", at(6))
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        store
            .apply_pick_replayed(clear("r-1", "1001", 1), record(7), replay("k-2", 'c'))
            .await
            .unwrap(),
        KeyedWrite::Applied(PickWrite::Cleared)
    );
}

async fn malformed_picks_are_refused<S: CharacterStore + SettingsStore>(store: S) {
    seeded(&store).await;
    for bad in [
        set("", "1001", "c-1", 0),
        set(&"r".repeat(65), "1001", "c-1", 0),
    ] {
        assert!(matches!(
            store.apply_pick(bad, record(4)).await,
            Err(StoreError::Constraint(_))
        ));
    }
    assert!(history(&store).await.is_empty());
}
