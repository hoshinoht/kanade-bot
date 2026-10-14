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
