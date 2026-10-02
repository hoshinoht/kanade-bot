//! The synthetic guild: fake members, fake parties, weekly timings and the
//! runs materialised from them. Boss keys are the real catalog's.

use super::catalog::{BossRef, boss_ref};
use super::dto::Participant;

pub struct MemberSeed {
    pub id: &'static str,
    pub name: &'static str,
    pub nickname: Option<&'static str>,
    pub aliases: &'static [&'static str],
    pub ping_level: &'static str,
    pub persona: Option<&'static str>,
    /// Holds the bossing role (the roster); others appear for chatbot access only.
    pub bossing: bool,
    /// Chatbot access from Discord roles: `staff`, `pilot` or `none`.
    pub access: &'static str,
}

const fn m(id: &'static str, name: &'static str, aliases: &'static [&'static str]) -> MemberSeed {
    MemberSeed {
        id,
        name,
        nickname: None,
        aliases,
        ping_level: "essential",
        persona: None,
        bossing: true,
        access: "none",
    }
}

pub fn members() -> Vec<MemberSeed> {
    vec![
        MemberSeed {
            access: "staff",
            persona: Some("kanade"),
            nickname: Some("asa"),
            ..m("1001", "Asahi", &["asa", "asahi"])
        },
        MemberSeed {
            access: "pilot",
            ..m("1002", "Ren", &["ren"])
        },
        MemberSeed {
            ping_level: "all",
            ..m("1003", "Mika", &["mika", "mk"])
        },
        m("1004", "Yuzu", &["yuzu"]),
        MemberSeed {
            ping_level: "off",
            ..m("1005", "Tsubame", &["tsu", "tsubame"])
        },
        m("1006", "Hinata", &["hina"]),
        MemberSeed {
            persona: Some("terse"),
            access: "pilot",
            ..m("1007", "Nagi", &["nagi"])
        },
        m("1008", "Sora", &["sora"]),
        m("1009", "Kaito", &["kai", "kaito"]),
        // A reply style that the persona catalog no longer offers.
        MemberSeed {
            persona: Some("sparkly"),
            access: "pilot",
            ..m("1010", "Rin", &["rin"])
        },
        m("1011", "Hotaru", &["hota", "hotaru"]),
        m("1012", "Minato", &["mina"]),
        // A second, different member who also goes by "Ren".
        MemberSeed {
            nickname: Some("ren2"),
            ..m("1013", "Ren", &["ren2"])
        },
        // Chatbot access without the bossing role: listed, never on a run.
        MemberSeed {
            bossing: false,
            access: "pilot",
            ..m("1014", "Kohane", &[])
        },
    ]
}

pub fn member_name(id: &str) -> Option<(&'static str, &'static str)> {
    members()
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| (s.id, s.name))
}

pub const PERSONAS: [(&str, &str); 3] = [
    ("default", "Default"),
    ("kanade", "Kanade"),
    ("terse", "Terse"),
];

/// Party channels: (id, display name, watched by the extractor).
pub const CHANNELS: [(&str, &str, bool); 9] = [
    ("baldrix-crew", "#baldrix-crew", true),
    ("kalos-four", "#kalos-four", true),
    ("limbo-trio", "#limbo-trio", true),
    ("fa-night", "#fa-night", true),
    ("jupiter-trio", "#jupiter-trio", true),
    ("bellona-otot", "#bellona-otot", true),
    ("hstar-party", "#hstar-party", true),
    ("bm-trio", "#bm-trio", false),
    ("seren-trio", "#seren-trio", true),
];

pub fn channel(id: &str) -> Option<(&'static str, &'static str, bool)> {
    CHANNELS.iter().copied().find(|c| c.0 == id)
}

pub struct Fixed {
    pub id: String,
    pub short_id: String,
    /// 0 = Monday, as v4's `weekday_names`.
    pub weekday: u8,
    pub time: String,
    pub bosses: Vec<BossRef>,
    pub participants: Vec<&'static str>,
    pub channel: &'static str,
    pub note: Option<String>,
    /// The owner's member id (proposal approval and chat authority accept them).
    pub owner_id: &'static str,
    pub retired: bool,
}

pub struct Rec {
    pub id: String,
    pub short_id: String,
    pub next_week: bool,
    pub day: u8,
    pub time: Option<String>,
    pub status: &'static str,
    pub bosses: Vec<BossRef>,
    pub participants: Vec<Participant>,
    pub channel: &'static str,
    pub fixed_id: Option<String>,
}

/// Monday-based weekday to the boss week's day index (Thursday reset).
pub fn day_of(weekday: u8) -> u8 {
    (weekday + 4) % 7
}

fn bosses(tokens: &str) -> Vec<BossRef> {
    tokens.split_whitespace().filter_map(boss_ref).collect()
}

pub fn people(list: &[(&str, &'static str)]) -> Vec<Participant> {
    list.iter()
        .filter_map(|&(id, answer)| {
            member_name(id).map(|(id, name)| Participant { id, name, answer })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn fixed(
    id: &str,
    short: &str,
    weekday: u8,
    time: &str,
    tokens: &str,
    members: &[&'static str],
    channel: &'static str,
    owner_id: &'static str,
) -> Fixed {
    Fixed {
        id: id.into(),
        short_id: short.into(),
        weekday,
        time: time.into(),
        bosses: bosses(tokens),
        participants: members.to_vec(),
        channel,
        note: None,
        owner_id,
        retired: false,
    }
}

pub fn fixed_runs() -> Vec<Fixed> {
    vec![
        fixed(
            "f-baldrix",
            "f1a2b3c4",
            3,
            "21:30",
            "NBaldrix",
            &["1001", "1002", "1003", "1004"],
            "baldrix-crew",
            "1001",
        ),
        // Runs at 22:00 this week: an amended run.
        fixed(
            "f-kalos",
            "f5d6e7f8",
            4,
            "21:30",
            "XKalos",
            &["1001", "1002", "1005", "1006"],
            "kalos-four",
            "1002",
        ),
        fixed(
            "f-limbo",
            "f9a0b1c2",
            4,
            "23:30",
            "HLimbo",
            &["1003", "1007", "1008"],
            "limbo-trio",
            "1003",
        ),
        fixed(
            "f-fa",
            "f3d4e5f6",
            0,
            "20:00",
            "HFA",
            &["1009", "1004", "1010", "1011"],
            "fa-night",
            "1009",
        ),
        fixed(
            "f-jupiter",
            "f7a8b9c0",
            0,
            "21:00",
            "HJupiter",
            &["1012", "1001", "1008"],
            "jupiter-trio",
            "1012",
        ),
        fixed(
            "f-carling",
            "fd1e2f3a",
            1,
            "22:00",
            "HCarling HStar",
            &["1001", "1002", "1003", "1004", "1005", "1011"],
            "hstar-party",
            "1001",
        ),
        fixed(
            "f-bm",
            "f4b5c6d7",
            1,
            "23:30",
            "XBM",
            &["1012", "1009", "1008"],
            "bm-trio",
            "1012",
        ),
        fixed(
            "f-seren",
            "f8e9f0a1",
            2,
            "21:00",
            "HSeren",
            &["1010", "1007", "1006"],
            "seren-trio",
            "1010",
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn rec(
    id: &str,
    short_id: &str,
    next_week: bool,
    day: u8,
    time: Option<&str>,
    status: &'static str,
    tokens: &str,
    participants: Vec<Participant>,
    channel: &'static str,
    fixed_id: Option<&str>,
) -> Rec {
    Rec {
        id: id.into(),
        short_id: short_id.into(),
        next_week,
        day,
        time: time.map(Into::into),
        status,
        bosses: bosses(tokens),
        participants,
        channel,
        fixed_id: fixed_id.map(Into::into),
    }
}

pub fn runs() -> Vec<Rec> {
    let w = "waiting";
    vec![
        rec(
            "r-baldrix",
            "1b2c3d4e",
            false,
            0,
            Some("21:30"),
            "done",
            "NBaldrix",
            people(&[
                ("1001", "yes"),
                ("1002", "yes"),
                ("1003", "yes"),
                ("1004", "yes"),
            ]),
            "baldrix-crew",
            Some("f-baldrix"),
        ),
        rec(
            "r-kalos",
            "5a6b7c8d",
            false,
            1,
            Some("22:00"),
            "at_risk",
            "XKalos",
            people(&[
                ("1001", "yes"),
                ("1002", "yes"),
                ("1005", "no"),
                ("1006", "yes"),
            ]),
            "kalos-four",
            Some("f-kalos"),
        ),
        rec(
            "r-limbo",
            "9e0f1a2b",
            false,
            1,
            Some("23:30"),
            "planned",
            "HLimbo",
            people(&[("1003", "yes"), ("1007", "yes"), ("1008", w)]),
            "limbo-trio",
            Some("f-limbo"),
        ),
        rec(
            "r-fa",
            "3c4d5e6f",
            false,
            4,
            Some("20:00"),
            "planned",
            "HFA",
            people(&[
                ("1009", "yes"),
                ("1004", "yes"),
                ("1010", "yes"),
                ("1011", w),
            ]),
            "fa-night",
            Some("f-fa"),
        ),
        rec(
            "r-jupiter",
            "7a8b9c0d",
            false,
            4,
            Some("21:00"),
            "confirmed",
            "HJupiter",
            people(&[("1012", "yes"), ("1001", "yes"), ("1008", "yes")]),
            "jupiter-trio",
            Some("f-jupiter"),
        ),
        // Added from chat, not from a weekly timing.
        rec(
            "r-bellona",
            "1e2f3a4b",
            false,
            4,
            None,
            "otot",
            "NBellona",
            people(&[
                ("1007", "yes"),
                ("1006", "yes"),
                ("1010", "maybe"),
                ("1009", w),
            ]),
            "bellona-otot",
            None,
        ),
        // One extra member this week: an amended roster (+Ren #1013).
        rec(
            "r-carling",
            "630b3544",
            false,
            5,
            Some("22:00"),
            "planned",
            "HCarling HStar",
            people(&[
                ("1001", "yes"),
                ("1002", "yes"),
                ("1003", "yes"),
                ("1004", "yes"),
                ("1005", w),
                ("1011", "maybe"),
                ("1013", "maybe"),
            ]),
            "hstar-party",
            Some("f-carling"),
        ),
        rec(
            "r-bm",
            "5c6d7e8f",
            false,
            5,
            Some("23:30"),
            "planned",
            "XBM",
            people(&[("1012", "yes"), ("1009", "yes"), ("1008", w)]),
            "bm-trio",
            Some("f-bm"),
        ),
        rec(
            "r-seren",
            "9a0b1c2d",
            false,
            6,
            Some("21:00"),
            "cancelled",
            "HSeren",
            people(&[("1010", "no"), ("1007", "no"), ("1006", w)]),
            "seren-trio",
            Some("f-seren"),
        ),
        rec(
            "n-kalos",
            "a1b2c3d4",
            true,
            1,
            Some("21:30"),
            "planned",
            "XKalos",
            people(&[("1001", w), ("1002", w), ("1005", w), ("1006", w)]),
            "kalos-four",
            Some("f-kalos"),
        ),
        rec(
            "n-fa",
            "e5f6a7b8",
            true,
            4,
            Some("20:00"),
            "planned",
            "HFA",
            people(&[("1009", w), ("1004", w), ("1010", w), ("1011", w)]),
            "fa-night",
            Some("f-fa"),
        ),
        rec(
            "n-carling",
            "c9d0e1f2",
            true,
            5,
            Some("22:00"),
            "planned",
            "HCarling HStar",
            people(&[
                ("1001", w),
                ("1002", w),
                ("1003", w),
                ("1004", w),
                ("1005", w),
                ("1011", w),
            ]),
            "hstar-party",
            Some("f-carling"),
        ),
    ]
}
