//! Should the bot answer this message at all? (v4 `chat/gate.py`)
//!
//! Every check is a security boundary, so the whole decision is one pure
//! function over the message's resolved lists. The cheapest, least revealing
//! checks run first; the two budgets (the asker's window, then the guild
//! pool) run last and are both read before either is spent. Every refusal
//! except a spent budget is silent.

mod channels;
mod limiter;

use std::fmt;

pub use channels::{ChannelDirectory, ChannelInfo, PilotSettings, is_chat_channel};
pub use limiter::RateLimiter;

use channels::as_int;

/// Heard you, thinking.
pub const SEEN_REACTION: &str = "👀";
/// A budget is spent (the person's or the guild's).
pub const RATE_LIMITED_REACTION: &str = "⏳";
/// The model is busy here or elsewhere.
pub const CHANNEL_BUSY_REACTION: &str = "💬";

/// The guild-wide pool's single key.
pub const GLOBAL_KEY: &str = "guild";
pub const RATE_LIMITED: &str = "rate limited";
/// Kept distinct from [`RATE_LIMITED`] so a log says which budget ran out.
pub const POOL_SPENT: &str = "the guild's answer budget is spent";
/// A zero allowance (no override): silent, like the role gate, so nothing
/// about the member reaches the model or the log.
pub const STAFF_ONLY: &str = "the member allowance is staff only";

/// Static budget replies; composing them never costs a generation.
pub const RATE_LIMITED_REPLY: &str =
    "That's your {count} answer{plural} for now — ask me again in about {wait}.";
pub const POOL_SPENT_REPLY: &str =
    "The guild's used up its answers for the moment — try me again in about {wait}.";

/// Retry intervals above this many seconds are said in minutes.
const RETRY_SECONDS_UNTIL: u64 = 120;

/// Answer or not, and why; the reason is for the log, never for Discord.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatDecision {
    pub act: bool,
    pub reason: &'static str,
    /// Budget spent: react [`RATE_LIMITED_REACTION`] and drop.
    pub busy: bool,
    /// With `busy`, seconds until the refusing budget has room again.
    pub retry_after_s: f64,
}

impl ChatDecision {
    fn refuse(reason: &'static str) -> Self {
        Self {
            act: false,
            reason,
            busy: false,
            retry_after_s: 0.0,
        }
    }

    fn ok() -> Self {
        Self {
            act: true,
            reason: "ok",
            busy: false,
            retry_after_s: 0.0,
        }
    }

    fn spent(reason: &'static str, retry_after_s: f64) -> Self {
        Self {
            act: false,
            reason,
            busy: true,
            retry_after_s,
        }
    }
}

impl fmt::Display for ChatDecision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason)
    }
}

/// A message author with the role ids of the live member (none for a DM,
/// webhook or uncached user, so the role gate fails closed).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Author {
    pub id: String,
    pub bot: bool,
    pub roles: Vec<String>,
}

/// Only Discord's resolved lists; text never grants a mention or a role.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncomingMessage {
    pub author: Option<Author>,
    pub guild_id: Option<String>,
    pub channel: Option<ChannelInfo>,
    /// Resolved user mentions.
    pub mentions: Vec<String>,
    /// Resolved role mentions.
    pub role_mentions: Vec<String>,
}

/// The trusted facts around one message.
#[derive(Clone, Copy, Debug)]
pub struct Summons<'a> {
    pub bot_user_id: Option<&'a str>,
    /// The bot's own managed integration role.
    pub self_role_id: Option<&'a str>,
    /// Who the message replies to, resolved by the caller.
    pub replied_author_id: Option<&'a str>,
    /// `chat_mode` is on.
    pub enabled: bool,
    pub is_admin: bool,
}

/// Was the bot mentioned (user mention, its managed role) or replied to?
pub fn mentions_bot(
    message: &IncomingMessage,
    bot_user_id: Option<&str>,
    self_role_id: Option<&str>,
    replied_author_id: Option<&str>,
) -> bool {
    let Some(wanted) = bot_user_id else {
        return false;
    };
    if message.mentions.iter().any(|id| id == wanted) {
        return true;
    }
    if let Some(managed) = self_role_id
        && message.role_mentions.iter().any(|id| id == managed)
    {
        return true;
    }
    replied_author_id == Some(wanted)
}

fn before_the_mention_check(
    message: &IncomingMessage,
    settings: &PilotSettings,
    directory: &(impl ChannelDirectory + ?Sized),
    bot_user_id: Option<&str>,
    enabled: bool,
) -> Option<ChatDecision> {
    let Some(author) = message.author.as_ref().filter(|author| !author.bot) else {
        return Some(ChatDecision::refuse("the author is a bot"));
    };
    if bot_user_id.is_some_and(|bot| author.id == bot) {
        return Some(ChatDecision::refuse("the bot's own message"));
    }
    let Some(guild) = &message.guild_id else {
        return Some(ChatDecision::refuse("not a guild message"));
    };
    if as_int(guild).unwrap_or(0) != as_int(&settings.guild_id).unwrap_or(0) {
        return Some(ChatDecision::refuse("another guild"));
    }
    if !enabled {
        return Some(ChatDecision::refuse("chat_mode is off"));
    }
    if !settings.configured() {
        return Some(ChatDecision::refuse("the chat pilot is not configured"));
    }
    if !is_chat_channel(message.channel.as_ref(), directory, settings) {
        return Some(ChatDecision::refuse("not a chat channel"));
    }
    None
}

/// Would this message reach the mention test? Asked before spending an API
/// call resolving a reply.
pub fn would_check_mention(
    message: &IncomingMessage,
    settings: &PilotSettings,
    directory: &(impl ChannelDirectory + ?Sized),
    bot_user_id: Option<&str>,
    enabled: bool,
) -> bool {
    before_the_mention_check(message, settings, directory, bot_user_id, enabled).is_none()
}

/// The non-spending access checks.
pub fn access_decide(
    message: &IncomingMessage,
    settings: &PilotSettings,
    directory: &(impl ChannelDirectory + ?Sized),
    summons: Summons<'_>,
) -> ChatDecision {
    if let Some(early) = before_the_mention_check(
        message,
        settings,
        directory,
        summons.bot_user_id,
        summons.enabled,
    ) {
        return early;
    }
    if !mentions_bot(
        message,
        summons.bot_user_id,
        summons.self_role_id,
        summons.replied_author_id,
    ) {
        return ChatDecision::refuse("the bot was not mentioned");
    }
    let holds_role = match (&settings.role_id, &message.author) {
        (Some(role), Some(author)) => {
            let role = as_int(role);
            role.is_some() && author.roles.iter().any(|held| as_int(held) == role)
        }
        _ => false,
    };
    if !holds_role && !summons.is_admin {
        return ChatDecision::refuse("the author does not hold the chat role");
    }
    ChatDecision::ok()
}

/// The asker's window and the guild pool; admins are exempt.
pub struct Budgets<'a> {
    pub person: Option<&'a mut RateLimiter>,
    pub pool: Option<&'a mut RateLimiter>,
    /// Monotonic seconds.
    pub now: f64,
}

/// Whether to answer, spending one answer from each budget when it does.
pub fn decide(
    message: &IncomingMessage,
    settings: &PilotSettings,
    directory: &(impl ChannelDirectory + ?Sized),
    summons: Summons<'_>,
    budgets: Budgets<'_>,
) -> ChatDecision {
    let decision = access_decide(message, settings, directory, summons);
    if !decision.act || summons.is_admin {
        return decision;
    }
    let author = message
        .author
        .as_ref()
        .map_or("", |author| author.id.as_str());
    let Budgets { person, pool, now } = budgets;
    if let Some(person) = person.as_deref()
        && person.limit_for(author).0 == 0
    {
        return ChatDecision::refuse(STAFF_ONLY);
    }
    if let Some(person) = person.as_deref()
        && person.remaining(author, now) == 0
    {
        return ChatDecision::spent(RATE_LIMITED, person.retry_after(author, now));
    }
    if let Some(pool) = pool.as_deref()
        && pool.remaining(GLOBAL_KEY, now) == 0
    {
        return ChatDecision::spent(POOL_SPENT, pool.retry_after(GLOBAL_KEY, now));
    }
    if let Some(person) = person {
        person.allow(author, now);
    }
    if let Some(pool) = pool {
        pool.allow(GLOBAL_KEY, now);
    }
    ChatDecision::ok()
}

/// Who runs this bot: a server admin, the owner, or the configured role.
pub fn is_bot_admin(
    is_guild_admin: bool,
    is_guild_owner: bool,
    role_ids: &[String],
    admin_role_id: Option<&str>,
) -> bool {
    if is_guild_admin || is_guild_owner {
        return true;
    }
    let Some(admin) = admin_role_id.and_then(as_int) else {
        return false;
    };
    role_ids.iter().any(|role| as_int(role) == Some(admin))
}

/// A retry interval rounded up: `59.5` → `60s`, `121` → `3 min`.
pub fn retry_note(seconds: f64) -> String {
    // Saturating float → int; negative and NaN become 0 and then 1.
    let whole = (seconds.ceil() as u64).max(1);
    if whole <= RETRY_SECONDS_UNTIL {
        format!("{whole}s")
    } else {
        format!("{} min", whole.div_ceil(60))
    }
}
