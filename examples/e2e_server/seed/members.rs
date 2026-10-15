//! The guild: its roles, channels and members (mock `seed::members`,
//! `seed::CHANNELS`, `/api/admin/roles`), and the fake Discord users
//! `e2e/harness/member.spec` signs in as.

use kanade::domain::members::{Member, MemberProfile, MemberStore, PingLevel};

use super::{Seed, ids::*};
use crate::{Fail, fakes::discord};

pub const STAFF_ROLE: &str = "300001";
pub const BOSSING_ROLE: &str = "300003";
/// The mock lists chatbot pilots but configures no pilot role.
pub const PILOT_ROLE: &str = "300004";

/// The guild roles `/api/admin/roles` lists: id, name, `0xRRGGBB` colour.
pub const ROLES: [(&str, &str, Option<u32>); 3] = [
    (STAFF_ROLE, "staff", Some(0x00e0_a458)),
    (BOSSING_ROLE, "bossers", Some(0x005b_8def)),
    ("300002", "newbies", None),
];

/// A guild text channel and what the bot may do there.
pub struct Channel {
    pub id: &'static str,
    /// Its name, without the `#`.
    pub name: &'static str,
    /// The extractor reads it.
    pub watched: bool,
    pub send: bool,
    pub manage: bool,
}

const fn party(id: &'static str, name: &'static str) -> Channel {
    Channel {
        id,
        name,
        watched: true,
        send: true,
        manage: true,
    }
}

/// The party channels, then the digest channel.
pub const CHANNELS: [Channel; 10] = [
    party(BALDRIX_CREW, "baldrix-crew"),
    party(KALOS_FOUR, "kalos-four"),
    party(LIMBO_TRIO, "limbo-trio"),
    party(FA_NIGHT, "fa-night"),
    party(JUPITER_TRIO, "jupiter-trio"),
    party(BELLONA_OTOT, "bellona-otot"),
    party(HSTAR_PARTY, "hstar-party"),
    Channel {
        watched: false,
        send: false,
        manage: false,
        ..party(BM_TRIO, "bm-trio")
    },
    Channel {
        manage: false,
        ..party(SEREN_TRIO, "seren-trio")
    },
    Channel {
        watched: false,
        ..party(BOSS_SCHEDULE, "boss-schedule")
    },
];

/// A seeded channel's name (`#` included), else its id.
pub fn channel_name(id: &str) -> String {
    CHANNELS
        .iter()
        .find(|channel| channel.id == id)
        .map_or_else(|| id.to_owned(), |channel| format!("#{}", channel.name))
}

/// The watched channels' ids, as `KANADE_WATCH_CHANNEL_IDS` lists them.
pub fn watched_ids() -> String {
    CHANNELS
        .iter()
        .filter(|channel| channel.watched)
        .map(|channel| channel.id)
        .collect::<Vec<_>>()
        .join(",")
}

/// Chatbot access from Discord roles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Access {
    Staff,
    Pilot,
    None,
}

struct Row {
    id: &'static str,
    name: &'static str,
    aliases: &'static [&'static str],
    ping: PingLevel,
    persona: Option<&'static str>,
    /// Holds the bossing role; the others appear for chatbot access only.
    bossing: bool,
    access: Access,
}

const fn m(id: &'static str, name: &'static str, aliases: &'static [&'static str]) -> Row {
    Row {
        id,
        name,
        aliases,
        ping: PingLevel::Essential,
        persona: None,
        bossing: true,
        access: Access::None,
    }
}

/// The mock's members. Its nicknames (`asa` for Asahi, `ren2` for the second
/// Ren) are left out: the real name prefers a nickname, and every board
/// names them Asahi and Ren.
const MEMBERS: [Row; 14] = [
    Row {
        access: Access::Staff,
        persona: Some("kanade"),
        ..m(ASAHI, "Asahi", &["asa", "asahi"])
    },
    Row {
        access: Access::Pilot,
        ..m(REN, "Ren", &["ren"])
    },
    Row {
        ping: PingLevel::All,
        ..m(MIKA, "Mika", &["mika", "mk"])
    },
    m(YUZU, "Yuzu", &["yuzu"]),
    Row {
        ping: PingLevel::Off,
        ..m(TSUBAME, "Tsubame", &["tsu", "tsubame"])
    },
    m(HINATA, "Hinata", &["hina"]),
    Row {
        persona: Some("terse"),
        access: Access::Pilot,
        ..m(NAGI, "Nagi", &["nagi"])
    },
    m(SORA, "Sora", &["sora"]),
    m(KAITO, "Kaito", &["kai", "kaito"]),
    // A reply style the persona catalog no longer offers.
    Row {
        persona: Some("sparkly"),
        access: Access::Pilot,
        ..m(RIN, "Rin", &["rin"])
    },
    m(HOTARU, "Hotaru", &["hota", "hotaru"]),
    m(MINATO, "Minato", &["mina"]),
    // A second, different member who also goes by "Ren".
    m(REN2, "Ren", &["ren2"]),
    // Chatbot access without the bossing role: listed, never on a run.
    Row {
        bossing: false,
        access: Access::Pilot,
        ..m(KOHANE, "Kohane", &[])
    },
];

/// A seeded member's name.
pub fn member_name(id: &str) -> Option<&'static str> {
    MEMBERS.iter().find(|row| row.id == id).map(|row| row.name)
}

pub async fn seed(seed: &Seed<'_>) -> Result<(), Fail> {
    for row in &MEMBERS {
        let mut roles = Vec::new();
        if row.bossing {
            roles.push(BOSSING_ROLE.to_owned());
        }
        match row.access {
            Access::Staff => roles.push(STAFF_ROLE.to_owned()),
            Access::Pilot => roles.push(PILOT_ROLE.to_owned()),
            Access::None => {}
        }
        seed.store
            .put_member(MemberProfile {
                member: Member {
                    user_id: row.id.into(),
                    display_name: Some(row.name.into()),
                    nickname: None,
                    has_role: row.bossing,
                    is_bot: false,
                    ping_level: row.ping,
                },
                aliases: row
                    .aliases
                    .iter()
                    .map(|alias| (*alias).to_owned())
                    .collect(),
                reply_style: row.persona.map(str::to_owned),
                roles,
                is_guild_admin: false,
            })
            .await?;
    }
    // The fake Discord sign-in users member.spec reads.
    for (choice, has_role) in discord::MEMBERS {
        let user = discord::user(choice);
        seed.store
            .put_member(MemberProfile {
                member: Member {
                    user_id: user.id.clone(),
                    display_name: Some(user.display()),
                    has_role,
                    ..Member::default()
                },
                roles: if has_role {
                    vec![BOSSING_ROLE.to_owned()]
                } else {
                    Vec::new()
                },
                ..MemberProfile::default()
            })
            .await?;
    }
    Ok(())
}
