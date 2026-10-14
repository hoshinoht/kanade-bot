//! Module `roster`'s characters and picks (mock `characters::seed`,
//! `run_picks::seed`) and module `character_data`'s links and NEXON data
//! (mock `character_data::seed`), all invented.

use chrono::{DateTime, Duration, Utc};
use kanade::domain::{
    characters::{
        Character, CharacterChange, CharacterRecord, CharacterRole, CharacterStore, PickChange,
        PickScope,
    },
    chardata::{ConsentSurface, Link, LinkPut, LinkStatus, NOTICE_VERSION, Snapshot},
    history::{Actor, Surface},
};

use super::Seed;
use crate::Fail;

const DAY: i64 = 86_400;

/// Member, IGN, class, level, main, days since the last write, retired.
type Row = (
    &'static str,
    &'static str,
    &'static str,
    Option<u16>,
    bool,
    i64,
    bool,
);

/// Oldest first; ids `ch-s1`… in this order. Mika has three active and one
/// retired. The mock gives Sora two alts and Kaito one alt with no main;
/// the real roster keeps one active main per member, so their first
/// characters are the mains here.
const CHARACTERS: [Row; 13] = [
    ("1003", "MikaHero", "Hero", Some(260), false, 46, true),
    ("1003", "MikaNL", "Night Lord", Some(286), true, 2, false),
    ("1003", "MikaBish", "Bishop", Some(281), false, 2, false),
    ("1003", "Mikadere", "Lara", Some(266), false, 21, false),
    ("1001", "AsahiDK", "Dark Knight", Some(284), true, 5, false),
    ("1001", "AsahiMech", "Mechanic", Some(262), false, 12, false),
    ("1002", "RenShade", "Shade", Some(279), true, 3, false),
    ("1004", "YuzuPala", "Paladin", Some(270), true, 9, false),
    ("1005", "TsubaKain", "Kain", Some(255), true, 14, false),
    ("1011", "Hotaru7", "Adele", Some(275), true, 6, false),
    ("1008", "SoraPF", "Pathfinder", Some(279), true, 4, false),
    ("1008", "SoraLumi", "Luminous", Some(285), false, 4, false),
    ("1009", "KaitoLyn", "Lynn", None, true, 1, false),
];

/// The admin every seeded roster write is attributed to (Asahi, Discord).
fn asahi() -> Actor {
    Actor::admin("discord:1001")
}

/// NEXON's view of a linked character: name, class, level, combat power,
/// fetched seconds before the seed's now.
struct Data {
    name: &'static str,
    class: &'static str,
    level: u16,
    power: u64,
    fetched_ago: i64,
}

struct Linked {
    character: &'static str,
    member: &'static str,
    ign: &'static str,
    status: LinkStatus,
    linked_ago: i64,
    data: Option<Data>,
}

/// Today's batch ran at 04:01 Singapore, 7 h 59 min before the seed's now.
const BATCH: i64 = 7 * 3_600 + 59 * 60;

const fn linked(
    character: &'static str,
    member: &'static str,
    ign: &'static str,
    linked_days: i64,
    data: Option<Data>,
) -> Linked {
    Linked {
        character,
        member,
        ign,
        status: LinkStatus::Active,
        linked_ago: linked_days * DAY,
        data,
    }
}

const fn data(
    name: &'static str,
    class: &'static str,
    level: u16,
    power: u64,
    fetched_ago: i64,
) -> Option<Data> {
    Some(Data {
        name,
        class,
        level,
        power,
        fetched_ago,
    })
}

/// MikaBish, MikaNL, YuzuPala and TsubaKain linked, MikaHero linked but
/// retired, Mikadere stale, Hotaru7 linked with its data past 30 days (gone,
/// so it reads `expired`), AsahiDK pending (below), the rest not linked.
const LINKS: [Linked; 7] = [
    linked(
        "ch-s1",
        "1003",
        "MikaHero",
        56,
        data("MikaHero", "Hero", 261, 22_913_402, BATCH + 10 * DAY),
    ),
    linked(
        "ch-s2",
        "1003",
        "MikaNL",
        30,
        data("MikaNL", "Night Lord", 287, 52_640_118, DAY),
    ),
    linked(
        "ch-s3",
        "1003",
        "MikaBish",
        8,
        data("MikaBish", "Bishop", 283, 48_204_551, BATCH),
    ),
    Linked {
        status: LinkStatus::Stale,
        ..linked("ch-s4", "1003", "Mikadere", 20, None)
    },
    linked(
        "ch-s8",
        "1004",
        "YuzuPala",
        9,
        data("YuzuPala", "Paladin", 276, 52_104_311, BATCH),
    ),
    linked(
        "ch-s9",
        "1005",
        "TsubaKain",
        14,
        data("TsubaKain", "Kain", 281, 38_412_906, BATCH),
    ),
    linked("ch-s10", "1011", "Hotaru7", 40, None),
];

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    roster(seed).await?;
    picks(seed).await?;
    if let Some(service) = &seed.state.character_data {
        links(seed, service.store().as_ref()).await?;
    }
    Ok(())
}

async fn roster(seed: &Seed<'_>) -> Result<(), Fail> {
    let created = seed.ago(120 * DAY);
    for (n, (member, ign, class, level, main, days, retired)) in (0_i64..).zip(CHARACTERS) {
        let at = seed.ago(days * DAY);
        let created_at = created + Duration::hours(n);
        let row = Character {
            id: format!("ch-s{}", n + 1),
            member_id: member.into(),
            ign: ign.into(),
            world: "msea".into(),
            class: Some(class.into()),
            level,
            role: if main {
                CharacterRole::Main
            } else {
                CharacterRole::Alt
            },
            retired_at: retired.then_some(at),
            created_at,
            updated_at: at,
            updated_by: asahi(),
            version: 1,
        };
        let record = CharacterRecord {
            at: created_at,
            actor: asahi(),
            surface: Surface::AdminPortal,
        };
        let written = seed
            .store
            .apply_character(CharacterChange::Create(row), record)
            .await?;
        if !written.applied() {
            return Err(format!("character {ign} was refused: {written:?}").into());
        }
    }
    Ok(())
}

/// Asahi picks AsahiDK on `r-carling`; Mika's pick for the `f-carling`
/// timing is MikaBish, so its runs bring her from the timing.
async fn picks(seed: &Seed<'_>) -> Result<(), Fail> {
    let picks = [
        (PickScope::Run, "r-carling", "1001", "ch-s5", 2 * DAY),
        (PickScope::Timing, "f-carling", "1003", "ch-s3", DAY),
    ];
    for (scope, target, member, character, ago) in picks {
        let record = CharacterRecord {
            at: seed.ago(ago),
            actor: asahi(),
            surface: Surface::AdminPortal,
        };
        let change = PickChange::Set {
            scope,
            target: target.into(),
            member_id: member.into(),
            character_id: character.into(),
            version: 0,
        };
        let written = seed.store.apply_pick(change, record).await?;
        if !written.applied() {
            return Err(format!("the pick on {target} was refused: {written:?}").into());
        }
    }
    Ok(())
}

async fn put_link(
    store: &dyn kanade::domain::chardata::CharDataStore,
    link: Link,
    expected: u32,
) -> Result<(), Fail> {
    match store.put_link(link, expected).await? {
        LinkPut::Written => Ok(()),
        LinkPut::Stale(_) => Err("a seeded link was stale".into()),
    }
}

async fn links(
    seed: &Seed<'_>,
    store: &dyn kanade::domain::chardata::CharDataStore,
) -> Result<(), Fail> {
    // AsahiDK: an admin asked 20 minutes ago; Asahi has not confirmed.
    put_link(
        store,
        Link {
            character_id: "ch-s5".into(),
            member_id: "1001".into(),
            ign_key: "asahidk".into(),
            region: "msea".into(),
            status: LinkStatus::Pending,
            requested_by: asahi(),
            requested_at: seed.ago(20 * 60),
            consent_version: None,
            consented_at: None,
            linked_at: None,
            consent_surface: None,
            version: 1,
        },
        0,
    )
    .await?;
    for link in &LINKS {
        let linked_at = seed.ago(link.linked_ago);
        let pending = Link {
            character_id: link.character.into(),
            member_id: link.member.into(),
            ign_key: link.ign.to_lowercase(),
            region: "msea".into(),
            status: LinkStatus::Pending,
            requested_by: Actor::member(link.member),
            requested_at: linked_at - Duration::minutes(10),
            consent_version: None,
            consented_at: None,
            linked_at: None,
            consent_surface: None,
            version: 1,
        };
        put_link(store, pending.clone(), 0).await?;
        // Confirmed in Discord (the mock's stale link is at version 2 too).
        let confirmed = Link {
            consent_version: Some(NOTICE_VERSION),
            consented_at: Some(linked_at),
            linked_at: Some(linked_at),
            consent_surface: Some(ConsentSurface::Discord),
            ..pending.with_status(link.status)
        };
        put_link(store, confirmed, 1).await?;
        if let Some(data) = &link.data {
            let fetched_at: DateTime<Utc> = seed.ago(data.fetched_ago);
            let written = store
                .put_snapshot(Snapshot {
                    character_id: link.character.into(),
                    ocid: format!("ocid-{}", link.character),
                    ocid_fetched_at: fetched_at,
                    character_name: Some(data.name.into()),
                    world_name: Some("Aquila".into()),
                    character_class: Some(data.class.into()),
                    level: Some(data.level),
                    combat_power: Some(data.power),
                    image_url: None,
                    data_date: None,
                    fetched_at,
                    expires_at: Snapshot::expiry(fetched_at, fetched_at),
                })
                .await?;
            if !written {
                return Err(format!("{}'s data was refused", link.ign).into());
            }
        }
    }
    Ok(())
}
