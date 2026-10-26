//! Character picks every store must keep, per run and per weekly timing:
//! one stored pick per `(scope, target, member)`, and one per later boss
//! per `(scope, target, member, boss)`, each behind its own version (0
//! with none); a set names an active character of that member; a clear
//! deletes the row; "same for every boss" deletes the named per-boss rows
//! in one write, or none when any changed; a no-op or refusal writes and
//! records nothing; every effective write is recorded in History (section
//! `characters`, key `pick.<run>.<member>` or `fixedpick.<fixed>.<member>`,
//! `.<boss>` per boss, `.*` for "same for every boss") with it; keyed
//! writes replay in the same transaction; the two scopes never see each
//! other's rows; deleting a character drops its picks in both.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeZone, Utc};

use super::fixtures::refused;
use crate::infrastructure::store::replays::{KeyedWrite, ReplayScope, ReplayStore, StoredReplay};
use kanade_domain::characters::{
    CHARACTERS_SECTION, Character, CharacterChange, CharacterEdit, CharacterRecord, CharacterRole,
    CharacterStore, Pick, PickChange, PickScope, PickWrite,
};
use kanade_domain::history::{Actor, Surface};
use kanade_domain::settings::{SettingsChange, SettingsChangeQuery, SettingsStore};

const SCOPES: [PickScope; 2] = [PickScope::Run, PickScope::Timing];

pub async fn run_suite<S: CharacterStore + ReplayStore + SettingsStore>(make: impl AsyncFn() -> S) {
    for scope in SCOPES {
        picks_are_set_changed_and_cleared_behind_a_version(make().await, scope).await;
        picks_name_an_active_character_of_the_member(make().await, scope).await;
        picks_are_read_by_target(make().await, scope).await;
        deleting_a_character_drops_its_picks(make().await, scope).await;
        keyed_picks_replay_in_the_same_write(make().await, scope).await;
        malformed_picks_are_refused(make().await, scope).await;
        per_boss_picks_are_slots_of_their_own(make().await, scope).await;
        same_for_every_boss_clears_the_named_bosses_at_once(make().await, scope).await;
    }
    run_and_timing_picks_are_separate_slots(make().await).await;
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

fn set(scope: PickScope, target: &str, member: &str, character: &str, version: u32) -> PickChange {
    PickChange::Set {
        scope,
        target: target.into(),
        member_id: member.into(),
        boss: None,
        character_id: character.into(),
        version,
    }
}

fn clear(scope: PickScope, target: &str, member: &str, version: u32) -> PickChange {
    PickChange::Clear {
        scope,
        target: target.into(),
        member_id: member.into(),
        boss: None,
        version,
    }
}

/// `change` (a set or clear) for `boss` instead of the slot's own pick.
fn for_boss(change: PickChange, boss: &str) -> PickChange {
    match change {
        PickChange::Set {
            scope,
            target,
            member_id,
            character_id,
            version,
            ..
        } => PickChange::Set {
            scope,
            target,
            member_id,
            boss: Some(boss.into()),
            character_id,
            version,
        },
        PickChange::Clear {
            scope,
            target,
            member_id,
            version,
            ..
        } => PickChange::Clear {
            scope,
            target,
            member_id,
            boss: Some(boss.into()),
            version,
        },
        same @ PickChange::Same { .. } => same,
    }
}

fn same(scope: PickScope, target: &str, member: &str, versions: &[(&str, u32)]) -> PickChange {
    PickChange::Same {
        scope,
        target: target.into(),
        member_id: member.into(),
        versions: versions
            .iter()
            .map(|(boss, version)| ((*boss).to_owned(), *version))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn pick(
    scope: PickScope,
    target: &str,
    member: &str,
    character: &str,
    hour: u32,
    version: u32,
) -> Pick {
    Pick {
        scope,
        target: target.into(),
        member_id: member.into(),
        boss: None,
        character_id: character.into(),
        picked_at: at(hour),
        picked_by: admin(),
        version,
    }
}

fn boss_pick(scope: PickScope, boss: &str, character: &str, hour: u32, version: u32) -> Pick {
    Pick {
        boss: Some(boss.into()),
        ..pick(scope, "t-1", "1001", character, hour, version)
    }
}

async fn picks<S: CharacterStore>(store: &S, scope: PickScope, targets: &[&str]) -> Vec<Pick> {
    let targets: Vec<String> = targets.iter().map(|id| (*id).to_owned()).collect();
    store.picks(scope, &targets).await.expect("picks")
}

/// The History key prefix of `scope`'s picks.
fn prefix(scope: PickScope) -> &'static str {
    match scope {
        PickScope::Run => "pick.",
        PickScope::Timing => "fixedpick.",
    }
}

/// The pick rows of History, newest first.
async fn history<S: SettingsStore>(store: &S, scope: PickScope) -> Vec<SettingsChange> {
    store
        .settings_changes(SettingsChangeQuery::default())
        .await
        .expect("history")
        .into_iter()
        .filter(|change| {
            change
                .values
                .keys()
                .any(|key| key.starts_with(prefix(scope)))
        })
        .collect()
}

async fn picks_are_set_changed_and_cleared_behind_a_version<S: CharacterStore + SettingsStore>(
    store: S,
    scope: PickScope,
) {
    seeded(&store).await;
    let first = pick(scope, "t-1", "1001", "c-1", 4, 1);
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-1", 0), record(4))
            .await
            .unwrap(),
        PickWrite::Picked(first.clone())
    );
    assert_eq!(
        picks(&store, scope, &["t-1"]).await,
        std::slice::from_ref(&first)
    );
    // The same character again is a no-op: no write, no record.
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-1", 1), record(5))
            .await
            .unwrap(),
        PickWrite::Picked(first.clone())
    );
    // A write from version 0 now finds version 1.
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-2", 0), record(5))
            .await
            .unwrap(),
        PickWrite::Stale(Some(first.clone()))
    );
    let second = pick(scope, "t-1", "1001", "c-2", 6, 2);
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-2", 1), record(6))
            .await
            .unwrap(),
        PickWrite::Picked(second.clone())
    );
    assert_eq!(picks(&store, scope, &["t-1"]).await, [second]);
    assert_eq!(
        store
            .apply_pick(clear(scope, "t-1", "1001", 2), record(7))
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert!(picks(&store, scope, &["t-1"]).await.is_empty());
    // Clearing nothing is a no-op at version 0.
    assert_eq!(
        store
            .apply_pick(clear(scope, "t-1", "1001", 0), record(8))
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert_eq!(
        store
            .apply_pick(clear(scope, "t-1", "1001", 1), record(8))
            .await
            .unwrap(),
        PickWrite::Stale(None)
    );
    // After a clear the next pick starts again at version 1.
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-2", 0), record(9))
            .await
            .unwrap(),
        PickWrite::Picked(pick(scope, "t-1", "1001", "c-2", 9, 1))
    );

    let saves = history(&store, scope).await;
    assert_eq!(saves.len(), 4, "set, change, clear, set");
    let key = format!("{}t-1.1001", prefix(scope));
    let cleared = &saves[1];
    assert_eq!(cleared.section, CHARACTERS_SECTION);
    assert_eq!(cleared.at, at(7));
    assert_eq!(cleared.actor, admin());
    assert_eq!(cleared.revision, 0);
    assert!(cleared.values[&key].from.contains(r#""ign":"Wisteria""#));
    assert_eq!(cleared.values[&key].to, "");
    let changed = &saves[2].values[&key];
    assert!(changed.from.contains(r#""character":"c-1""#), "{changed:?}");
    assert!(changed.to.contains(r#""character":"c-2""#), "{changed:?}");
    let created = &saves[3].values[&key];
    assert_eq!(created.from, "");
    assert!(created.to.contains(r#""ign":"Lilac""#), "{created:?}");
    let target = match scope {
        PickScope::Run => r#""run":"t-1""#,
        PickScope::Timing => r#""fixed":"t-1""#,
    };
    assert!(created.to.contains(target), "{created:?}");
}

async fn picks_name_an_active_character_of_the_member<S: CharacterStore + SettingsStore>(
    store: S,
    scope: PickScope,
) {
    seeded(&store).await;
    for (change, refused) in [
        (
            set(scope, "t-1", "1001", "c-9", 0),
            PickWrite::UnknownCharacter,
        ),
        (set(scope, "t-1", "1001", "c-3", 0), PickWrite::NotTheirs),
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
            .apply_pick(set(scope, "t-1", "1001", "c-2", 0), record(5))
            .await
            .unwrap(),
        PickWrite::Retired
    );
    assert!(picks(&store, scope, &["t-1"]).await.is_empty());
    assert!(
        history(&store, scope).await.is_empty(),
        "refusals record nothing"
    );
}

async fn picks_are_read_by_target<S: CharacterStore>(store: S, scope: PickScope) {
    seeded(&store).await;
    for change in [
        set(scope, "t-1", "1001", "c-1", 0),
        set(scope, "t-1", "1002", "c-3", 0),
        set(scope, "t-2", "1001", "c-2", 0),
        set(scope, "t-3", "1002", "c-3", 0),
    ] {
        assert!(store.apply_pick(change, record(4)).await.unwrap().applied());
    }
    let slots = |found: Vec<Pick>| -> Vec<(String, String, String)> {
        found
            .into_iter()
            .map(|pick| (pick.target, pick.member_id, pick.character_id))
            .collect()
    };
    let owned = |target: &str, member: &str, character: &str| {
        (target.to_owned(), member.to_owned(), character.to_owned())
    };
    assert_eq!(
        slots(picks(&store, scope, &["t-2", "t-1"]).await),
        [
            owned("t-1", "1001", "c-1"),
            owned("t-1", "1002", "c-3"),
            owned("t-2", "1001", "c-2"),
        ],
        "by target, then member"
    );
    assert!(picks(&store, scope, &["t-9"]).await.is_empty());
    assert!(picks(&store, scope, &[]).await.is_empty());
}

async fn deleting_a_character_drops_its_picks<S: CharacterStore + SettingsStore>(
    store: S,
    scope: PickScope,
) {
    seeded(&store).await;
    for change in [
        set(scope, "t-1", "1001", "c-2", 0),
        set(scope, "t-2", "1001", "c-1", 0),
    ] {
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
        picks(&store, scope, &["t-1", "t-2"]).await,
        [pick(scope, "t-2", "1001", "c-1", 4, 1)],
        "only the deleted character's pick goes"
    );
    // The slot is free again: a new pick starts at version 1.
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-1", 0), record(6))
            .await
            .unwrap(),
        PickWrite::Picked(pick(scope, "t-1", "1001", "c-1", 6, 1))
    );
}

async fn keyed_picks_replay_in_the_same_write<S: CharacterStore + ReplayStore + SettingsStore>(
    store: S,
    scope: PickScope,
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
    let first = pick(scope, "t-1", "1001", "c-1", 4, 1);
    assert_eq!(
        store
            .apply_pick_replayed(
                set(scope, "t-1", "1001", "c-1", 0),
                record(4),
                replay("k", 'a')
            )
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
            .apply_pick_replayed(
                set(scope, "t-1", "1001", "c-1", 0),
                record(5),
                replay("k", 'a')
            )
            .await
            .unwrap(),
        KeyedWrite::Replayed(found)
    );
    assert_eq!(
        store
            .apply_pick_replayed(
                set(scope, "t-1", "1001", "c-2", 1),
                record(5),
                replay("k", 'b')
            )
            .await
            .unwrap(),
        KeyedWrite::KeyTaken
    );
    assert_eq!(picks(&store, scope, &["t-1"]).await, [first]);
    assert_eq!(
        history(&store, scope).await.len(),
        1,
        "replays record nothing"
    );

    // A refusal stores no answer: the key applies again later.
    assert_eq!(
        store
            .apply_pick_replayed(
                set(scope, "t-1", "1001", "c-3", 1),
                record(6),
                replay("k-2", 'c')
            )
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
            .apply_pick_replayed(
                clear(scope, "t-1", "1001", 1),
                record(7),
                replay("k-2", 'c')
            )
            .await
            .unwrap(),
        KeyedWrite::Applied(PickWrite::Cleared)
    );
}

async fn malformed_picks_are_refused<S: CharacterStore + SettingsStore>(
    store: S,
    scope: PickScope,
) {
    seeded(&store).await;
    for bad in [
        set(scope, "", "1001", "c-1", 0),
        set(scope, &"t".repeat(65), "1001", "c-1", 0),
    ] {
        refused(store.apply_pick(bad, record(4)).await);
    }
    assert!(history(&store, scope).await.is_empty());
}

/// A per-boss pick is a slot of its own beside the member's pick: its own
/// version, row, History key (with the boss) and delete; the member's pick
/// never moves it.
async fn per_boss_picks_are_slots_of_their_own<S: CharacterStore + SettingsStore>(
    store: S,
    scope: PickScope,
) {
    seeded(&store).await;
    let star = boss_pick(scope, "MaleficStar", "c-2", 4, 1);
    assert_eq!(
        store
            .apply_pick(
                for_boss(set(scope, "t-1", "1001", "c-2", 0), "MaleficStar"),
                record(4)
            )
            .await
            .unwrap(),
        PickWrite::Picked(star.clone())
    );
    // The member's own pick is at version 0 still, and leaves the boss's.
    let own = pick(scope, "t-1", "1001", "c-1", 5, 1);
    assert_eq!(
        store
            .apply_pick(set(scope, "t-1", "1001", "c-1", 0), record(5))
            .await
            .unwrap(),
        PickWrite::Picked(own.clone())
    );
    assert_eq!(
        picks(&store, scope, &["t-1"]).await,
        [own.clone(), star.clone()],
        "the member's pick, then the per-boss ones"
    );
    // Each boss has its own version: another boss starts at 0.
    assert_eq!(
        store
            .apply_pick(
                for_boss(set(scope, "t-1", "1001", "c-1", 1), "MaleficStar"),
                record(6)
            )
            .await
            .unwrap(),
        PickWrite::Picked(boss_pick(scope, "MaleficStar", "c-1", 6, 2))
    );
    assert_eq!(
        store
            .apply_pick(
                for_boss(set(scope, "t-1", "1001", "c-2", 1), "Limbo"),
                record(6)
            )
            .await
            .unwrap(),
        PickWrite::Stale(None)
    );
    assert!(
        store
            .apply_pick(
                for_boss(set(scope, "t-1", "1001", "c-3", 0), "Limbo"),
                record(6)
            )
            .await
            .unwrap()
            == PickWrite::NotTheirs
    );
    // History: one row per effective write, keyed by the boss, its sides
    // naming it; the first per-boss pick came from "no pick of its own".
    let rows = history(&store, scope).await;
    let key = format!("{}t-1.1001.MaleficStar", prefix(scope));
    let boss_rows: Vec<_> = rows
        .iter()
        .filter_map(|change| change.values.get(&key))
        .collect();
    assert_eq!(boss_rows.len(), 2, "{rows:?}");
    assert_eq!(boss_rows[1].from, "");
    let to: serde_json::Value = serde_json::from_str(&boss_rows[1].to).unwrap();
    assert_eq!(to["boss"], "MaleficStar");
    assert_eq!(to["ign"], "Wisteria");
    let own_key = format!("{}t-1.1001", prefix(scope));
    let own_row = rows
        .iter()
        .find_map(|change| change.values.get(&own_key))
        .expect("the member's pick row");
    let own_side: serde_json::Value = serde_json::from_str(&own_row.to).unwrap();
    assert!(own_side.get("boss").is_none(), "{own_side}");
    // Clearing the boss's pick deletes its row only.
    assert_eq!(
        store
            .apply_pick(
                for_boss(clear(scope, "t-1", "1001", 2), "MaleficStar"),
                record(7)
            )
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert_eq!(picks(&store, scope, &["t-1"]).await, [own]);
    // Deleting a character drops its per-boss picks too.
    assert!(
        store
            .apply_pick(
                for_boss(set(scope, "t-1", "1001", "c-2", 0), "Limbo"),
                record(8)
            )
            .await
            .unwrap()
            .applied()
    );
    assert!(
        store
            .apply_character(
                CharacterChange::Delete {
                    id: "c-2".into(),
                    version: 1
                },
                record(9)
            )
            .await
            .unwrap()
            .applied()
    );
    assert_eq!(
        picks(&store, scope, &["t-1"]).await,
        [pick(scope, "t-1", "1001", "c-1", 5, 1)]
    );
    for bad in ["", &"b".repeat(65)] {
        refused(
            store
                .apply_pick(
                    for_boss(set(scope, "t-1", "1001", "c-1", 0), bad),
                    record(10),
                )
                .await,
        );
    }
}

/// "Same for every boss" deletes every per-boss pick it names in one write
/// with one History row (key `.*`, the cleared picks on its `from` side);
/// a boss at another version refuses the whole write; a boss it does not
/// name keeps its pick; with nothing to clear it records nothing.
async fn same_for_every_boss_clears_the_named_bosses_at_once<
    S: CharacterStore + ReplayStore + SettingsStore,
>(
    store: S,
    scope: PickScope,
) {
    seeded(&store).await;
    for (boss, character) in [("MaleficStar", "c-2"), ("Limbo", "c-2"), ("Kalos", "c-1")] {
        assert!(
            store
                .apply_pick(
                    for_boss(set(scope, "t-1", "1001", character, 0), boss),
                    record(4)
                )
                .await
                .unwrap()
                .applied()
        );
    }
    let before = history(&store, scope).await.len();
    // Limbo was read at another version: nothing goes.
    assert_eq!(
        store
            .apply_pick(
                same(scope, "t-1", "1001", &[("MaleficStar", 1), ("Limbo", 2)]),
                record(5)
            )
            .await
            .unwrap(),
        PickWrite::Stale(None)
    );
    assert_eq!(picks(&store, scope, &["t-1"]).await.len(), 3);
    let replay = StoredReplay::new(
        ReplayScope::Config,
        "admin:token".into(),
        "same".into(),
        std::iter::repeat_n('d', 64).collect(),
        200,
        r#"{"member_id":"1001"}"#.into(),
        at(5),
    );
    assert_eq!(
        store
            .apply_pick_replayed(
                same(scope, "t-1", "1001", &[("MaleficStar", 1), ("Limbo", 1)]),
                record(5),
                replay.clone()
            )
            .await
            .unwrap(),
        KeyedWrite::Applied(PickWrite::Cleared)
    );
    assert_eq!(
        picks(&store, scope, &["t-1"]).await,
        [boss_pick(scope, "Kalos", "c-1", 4, 1)],
        "a boss it did not name keeps its pick"
    );
    let rows = history(&store, scope).await;
    assert_eq!(rows.len(), before + 1, "one row for the whole write");
    let key = format!("{}t-1.1001.*", prefix(scope));
    let row = rows[0]
        .values
        .get(&key)
        .expect("the same-for-every-boss row");
    assert_eq!(rows[0].values.len(), 1);
    assert_eq!(row.to, "");
    let cleared: serde_json::Value = serde_json::from_str(&row.from).unwrap();
    let bosses: Vec<&str> = cleared
        .as_array()
        .unwrap()
        .iter()
        .map(|side| side["boss"].as_str().unwrap())
        .collect();
    assert_eq!(bosses, ["Limbo", "MaleficStar"]);
    assert_eq!(cleared[0]["ign"], "Wisteria");
    // The retry replays; nothing left to clear is a no-op with no row.
    assert!(matches!(
        store
            .apply_pick_replayed(
                same(scope, "t-1", "1001", &[("MaleficStar", 1), ("Limbo", 1)]),
                record(6),
                replay
            )
            .await
            .unwrap(),
        KeyedWrite::Replayed(_)
    ));
    assert_eq!(
        store
            .apply_pick(
                same(scope, "t-1", "1001", &[("MaleficStar", 0), ("Limbo", 0)]),
                record(7)
            )
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert_eq!(history(&store, scope).await.len(), before + 1);
}

/// A run and a timing may share an id: their slots, versions and History
/// keys stay apart, and a deleted character leaves neither behind.
async fn run_and_timing_picks_are_separate_slots<S: CharacterStore + SettingsStore>(store: S) {
    seeded(&store).await;
    let run = pick(PickScope::Run, "x-1", "1001", "c-1", 4, 1);
    let timing = pick(PickScope::Timing, "x-1", "1001", "c-2", 4, 1);
    for change in [
        set(PickScope::Run, "x-1", "1001", "c-1", 0),
        set(PickScope::Timing, "x-1", "1001", "c-2", 0),
    ] {
        assert!(store.apply_pick(change, record(4)).await.unwrap().applied());
    }
    assert_eq!(picks(&store, PickScope::Run, &["x-1"]).await, [run]);
    assert_eq!(
        picks(&store, PickScope::Timing, &["x-1"]).await,
        std::slice::from_ref(&timing)
    );
    // Clearing the run's pick leaves the timing's (at its own version).
    assert_eq!(
        store
            .apply_pick(clear(PickScope::Run, "x-1", "1001", 1), record(5))
            .await
            .unwrap(),
        PickWrite::Cleared
    );
    assert_eq!(
        picks(&store, PickScope::Timing, &["x-1"]).await,
        std::slice::from_ref(&timing)
    );
    assert_eq!(history(&store, PickScope::Run).await.len(), 2);
    assert_eq!(history(&store, PickScope::Timing).await.len(), 1);
    // The run picks Wisteria too; deleting her drops both slots.
    assert!(
        store
            .apply_pick(set(PickScope::Run, "x-1", "1001", "c-2", 0), record(6))
            .await
            .unwrap()
            .applied()
    );
    assert!(
        store
            .apply_character(
                CharacterChange::Delete {
                    id: "c-2".into(),
                    version: 1
                },
                record(7)
            )
            .await
            .unwrap()
            .applied()
    );
    assert!(picks(&store, PickScope::Run, &["x-1"]).await.is_empty());
    assert!(picks(&store, PickScope::Timing, &["x-1"]).await.is_empty());
}
