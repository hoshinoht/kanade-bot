//! Production pseudonymizing codec: member identities become fictional given
//! names drawn at random, per session, from a curated pool filtered against the
//! roster, the code lexicon and the bot/persona names. Rules are in the identity
//! section of `docs/v5/provider-contract.md`.

mod decode;
mod encode;
mod issuer;
mod json;
mod lexicon;
mod matcher;
mod normalize;
mod pool;
mod random;
mod session;
mod words;

use std::{collections::HashSet, fmt, sync::Arc};

use super::codec::{CodecMode, IdentityCodec, IdentitySession, Member};
use crate::infrastructure::llm::governor::Random;
use normalize::{name_keys, near_match, normalise};

pub use lexicon::CodeLexicon;
pub use pool::NamePool;
pub use random::SystemRng;
pub use session::PseudonymSession;

/// The bot itself: its id is never tokenised and its mentions render as
/// `@name`. `name` and `aliases` (persona names) are never issued as tokens.
#[derive(Clone, Default)]
pub struct BotIdentity {
    pub user_id: Option<String>,
    pub name: String,
    pub aliases: Vec<String>,
}

impl fmt::Debug for BotIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BotIdentity")
            .field("has_user_id", &self.user_id.is_some())
            .field("alias_count", &self.aliases.len())
            .finish()
    }
}

pub struct PseudonymConfig {
    pub pool: NamePool,
    pub lexicon: CodeLexicon,
    pub bot: BotIdentity,
    /// More names never to issue (near-match), e.g. names the persona prompt uses.
    pub extra_exclusions: Vec<String>,
    pub random: Arc<dyn Random>,
}

impl fmt::Debug for PseudonymConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PseudonymConfig")
            .field("pool", &self.pool)
            .field("lexicon", &self.lexicon)
            .field("bot", &self.bot)
            .field("extra_exclusions", &self.extra_exclusions.len())
            .finish()
    }
}

#[derive(Clone)]
pub(super) struct Candidate {
    name: String,
    /// Normalised name.
    key: String,
}

/// State shared by every session of one codec.
pub(super) struct Shared {
    /// Pool after construction-time filters, in list order.
    pool: Vec<Candidate>,
    /// Lowercase `pool` names.
    forms: Arc<HashSet<String>>,
    lexicon: CodeLexicon,
    bot: BotIdentity,
    random: Arc<dyn Random>,
}

pub struct PseudonymCodec {
    shared: Arc<Shared>,
}

impl PseudonymCodec {
    pub fn new(config: PseudonymConfig) -> Self {
        let blocked: Vec<String> = std::iter::once(&config.bot.name)
            .chain(&config.bot.aliases)
            .chain(&config.extra_exclusions)
            .flat_map(|name| name_keys(name))
            .collect();
        let pool: Vec<Candidate> = config
            .pool
            .names()
            .filter(|name| !config.lexicon.excludes(name))
            .map(|name| Candidate {
                name: name.to_owned(),
                key: normalise(name),
            })
            .filter(|c| !blocked.iter().any(|key| near_match(&c.key, key)))
            .collect();
        let forms = pool.iter().map(|c| c.name.to_ascii_lowercase()).collect();
        Self {
            shared: Arc::new(Shared {
                pool,
                forms: Arc::new(forms),
                lexicon: config.lexicon,
                bot: config.bot,
                random: config.random,
            }),
        }
    }

    /// Pool names left after construction-time filters (before any roster).
    pub fn pool_size(&self) -> usize {
        self.shared.pool.len()
    }

    /// A concrete session, for callers that need [`PseudonymSession`]'s own API.
    pub fn open_session(&self, roster: &[Member]) -> PseudonymSession {
        PseudonymSession::new(Arc::clone(&self.shared), roster)
    }
}

impl IdentityCodec for PseudonymCodec {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(self.open_session(roster))
    }
}

impl fmt::Debug for PseudonymCodec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PseudonymCodec")
            .field("pool", &self.shared.pool.len())
            .field("lexicon", &self.shared.lexicon)
            .field("bot", &self.shared.bot)
            .finish()
    }
}
