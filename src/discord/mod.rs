//! Discord over REST and the guild as the bot sees it, shared by the bot and
//! the web server: the outcome-classifying transport seam, the explicit
//! mention policy, the configured guild's scope and staff facts, the guild
//! cache, the bot's identity art and member portraits from the CDN, and the
//! member roster (its snapshot, store sink and reconciliation). Nothing here
//! opens a gateway connection.

pub mod avatars;
pub mod guild;
pub mod guild_cache;
pub mod identity;
pub mod mentions;
pub mod roster;
pub mod transport;
