//! The seeded Discord user ids. Real ids are 17-20 digit snowflakes (the
//! public hand-off refuses anything else), so each mock member id `10NN`
//! becomes the snowflake the mock's public origin already shows for it
//! (`portal::snowflake`: `1` then the id zero-padded to 17 digits).

pub const ASAHI: &str = "100000000000001001";
pub const REN: &str = "100000000000001002";
pub const MIKA: &str = "100000000000001003";
pub const YUZU: &str = "100000000000001004";
pub const TSUBAME: &str = "100000000000001005";
pub const HINATA: &str = "100000000000001006";
pub const NAGI: &str = "100000000000001007";
pub const SORA: &str = "100000000000001008";
pub const KAITO: &str = "100000000000001009";
pub const RIN: &str = "100000000000001010";
pub const HOTARU: &str = "100000000000001011";
pub const MINATO: &str = "100000000000001012";
/// The second, different Ren.
pub const REN2: &str = "100000000000001013";
pub const KOHANE: &str = "100000000000001014";

/// The fake Discord sign-in users (`fakes::discord`).
pub const MIKAN: &str = "100000000000000001";
pub const PLAIN: &str = "100000000000000002";
pub const BOTTY: &str = "100000000000000003";
pub const PARTNER: &str = "100000000000000004";

/// Someone who wrote in a watched channel but is not on the roster.
pub const STRANGER: &str = "114948601234567890";
/// The bot's own user (`fakes::guild`).
pub const BOT: &str = "1543532497948909578";

/// The guild's text channels (`members::CHANNELS`). Real channel ids are
/// snowflakes too (`KANADE_WATCH_CHANNEL_IDS` takes nothing else); each is
/// `12` then the channel's place in the mock's list, zero-padded to 18 digits.
pub const BALDRIX_CREW: &str = "120000000000000001";
pub const KALOS_FOUR: &str = "120000000000000002";
pub const LIMBO_TRIO: &str = "120000000000000003";
pub const FA_NIGHT: &str = "120000000000000004";
pub const JUPITER_TRIO: &str = "120000000000000005";
pub const BELLONA_OTOT: &str = "120000000000000006";
pub const HSTAR_PARTY: &str = "120000000000000007";
pub const BM_TRIO: &str = "120000000000000008";
pub const SEREN_TRIO: &str = "120000000000000009";
pub const BOSS_SCHEDULE: &str = "120000000000000010";
