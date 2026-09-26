//! Pseudonymization composition (`models.pseudonymize`): one live codec per
//! model role, built from the guild cache's bot identity, the persona's
//! names, the boss catalog and each role's own code-owned prompt text
//! (the boundary scanner's tailored exemptions, user decision: never the
//! broad `collides` flag). Off, every role is `Passthrough`.

use std::sync::{Arc, Mutex, PoisonError};

use chrono_tz::Tz;

use crate::{
    bot::{guild_cache::GuildCache, ids::parse_id, roster::LiveRoster},
    chat::{nudge, persona::PersonaStore, prompts, tools::ToolName},
    domain::catalog::BossTable,
    extract::prompt as extraction,
    infrastructure::llm::identity::{
        BotIdentity, CodeLexicon, CodecMode, IdentityCodec, IdentitySession, Member, MentionNames,
        NamePool, Passthrough, PseudonymCodec, PseudonymConfig, RosterSource, ScanExemptions,
        SystemRng,
    },
};

/// Every tool a chat request may offer (definitions are code-owned).
fn tool_definitions() -> ScanExemptions {
    ToolName::V4
        .into_iter()
        .chain([ToolName::RequestTools])
        .fold(ScanExemptions::default(), |words, tool| {
            words.with_value(&tool.schema())
        })
}

/// Chat's code-owned words: policies, cues, the clock header (its weekday
/// and month names, `zone`), tool definitions, and the boss catalog the
/// boss tools render.
pub fn chat_exemptions(zone: Tz, catalog: &BossTable) -> ScanExemptions {
    ScanExemptions::default()
        .with_texts(prompts::code_owned_texts(zone.name()))
        .with_boss_table(catalog)
        .extend(&tool_definitions())
}

/// Extraction's: the system prompt, the user prompt's headings, weekday
/// names, the retry instruction, the schema and the rendered BOSSES table.
pub fn extraction_exemptions(zone: Tz, catalog: &BossTable) -> ScanExemptions {
    ScanExemptions::default()
        .with_texts(extraction::code_owned_texts(zone, catalog))
        .with_value(&extraction::code_owned_schema())
        .with_boss_table(catalog)
}

/// The rewrite prompt's instruction, moods and voice label.
pub fn rewrite_exemptions() -> ScanExemptions {
    ScanExemptions::default().with_texts(nudge::code_owned_texts())
}

/// `<#id>` → `#name`, `<@&id>` → `@name` from the guild cache.
pub struct CacheNames(pub Arc<GuildCache>);

impl MentionNames for CacheNames {
    fn channel(&self, id: &str) -> Option<String> {
        self.0.channel_name(id)
    }

    fn role(&self, id: &str) -> Option<String> {
        let id = parse_id(id)?;
        self.0
            .role_names()
            .into_iter()
            .find(|(role, _)| *role == id)
            .map(|(_, role)| role.name)
    }
}

/// The live roster for the rewriter (bots excluded, like extraction's).
pub struct LiveRosterSource(pub Arc<LiveRoster>);

impl RosterSource for LiveRosterSource {
    fn roster(&self) -> Option<Vec<Member>> {
        Some(roster_members(&self.0))
    }
}

pub fn roster_members(roster: &LiveRoster) -> Vec<Member> {
    roster
        .profiles()
        .into_iter()
        .filter(|profile| !profile.member.is_bot)
        .map(|profile| Member {
            display_name: profile
                .member
                .display_name
                .clone()
                .unwrap_or_else(|| profile.member.user_id.clone()),
            user_id: profile.member.user_id,
            nickname: profile.member.nickname,
            aliases: profile.aliases,
        })
        .collect()
}

/// The persona's names (`# Persona:` of the active bundle), never issued.
pub fn persona_names(personas: &PersonaStore) -> Vec<String> {
    let snapshot = personas.pin();
    snapshot
        .active()
        .map(|active| {
            let name = crate::chat::persona::persona_name(&active.bundle.value.identity);
            vec![name.to_owned()]
        })
        .unwrap_or_default()
}

/// A pseudonymizing codec whose bot identity follows the guild cache (known
/// only after `READY`): rebuilt whenever the bot's id or names change.
pub struct LiveCodec {
    exemptions: ScanExemptions,
    lexicon: CodeLexicon,
    cache: Arc<GuildCache>,
    persona: Vec<String>,
    current: Mutex<Option<(Vec<String>, Arc<PseudonymCodec>)>>,
}

impl LiveCodec {
    pub fn new(
        exemptions: ScanExemptions,
        catalog: &BossTable,
        cache: Arc<GuildCache>,
        persona: Vec<String>,
    ) -> Self {
        Self {
            exemptions,
            lexicon: CodeLexicon::from_boss_table(catalog),
            cache,
            persona,
            current: Mutex::new(None),
        }
    }

    fn codec(&self) -> Arc<PseudonymCodec> {
        let user_id = self.cache.self_id().map(|id| id.get().to_string());
        let mut names = self.cache.self_names();
        for name in &self.persona {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        let key: Vec<String> = user_id
            .iter()
            .cloned()
            .chain(names.iter().cloned())
            .collect();
        let mut current = self.current.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((known, codec)) = current.as_ref()
            && *known == key
        {
            return Arc::clone(codec);
        }
        let mut names = names.into_iter();
        let bot = BotIdentity {
            user_id,
            name: names.next().unwrap_or_default(),
            aliases: names.collect(),
        };
        let codec = Arc::new(
            PseudonymCodec::new(PseudonymConfig {
                pool: NamePool::curated(),
                lexicon: self.lexicon.clone(),
                bot,
                extra_exclusions: self.persona.clone(),
                random: Arc::new(SystemRng::new()),
            })
            .with_scan_exemptions(&self.exemptions)
            .with_mention_names(Arc::new(CacheNames(Arc::clone(&self.cache)))),
        );
        *current = Some((key, Arc::clone(&codec)));
        codec
    }
}

impl IdentityCodec for LiveCodec {
    fn mode(&self) -> CodecMode {
        CodecMode::Pseudonymizing
    }

    fn open(&self, roster: &[Member]) -> Box<dyn IdentitySession> {
        self.codec().open(roster)
    }

    fn scan_exemptions(&self) -> Arc<ScanExemptions> {
        self.codec().scan_exemptions()
    }
}

impl std::fmt::Debug for LiveCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveCodec")
            .field("exemptions", &self.exemptions)
            .finish_non_exhaustive()
    }
}

/// Passthrough when masking is off; else a [`LiveCodec`].
pub fn codec(
    masking: bool,
    exemptions: impl FnOnce() -> ScanExemptions,
    catalog: &BossTable,
    cache: &Arc<GuildCache>,
    persona: Vec<String>,
) -> Arc<dyn IdentityCodec> {
    if masking {
        Arc::new(LiveCodec::new(
            exemptions(),
            catalog,
            Arc::clone(cache),
            persona,
        ))
    } else {
        Arc::new(Passthrough)
    }
}

#[cfg(test)]
mod tests {
    use twilight_model::id::Id;

    use super::*;
    use crate::domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};
    use crate::infrastructure::llm::identity::CodecMode;

    fn catalog() -> BossTable {
        BossTable::from_spec(&CatalogSpec {
            difficulties: vec![DifficultySpec {
                prefix: "H".into(),
                label: "Hard".into(),
            }],
            bosses: vec![BossSpec {
                short: "Will".into(),
                ..BossSpec::default()
            }],
        })
        .expect("catalog")
    }

    #[test]
    fn masking_picks_a_live_codec_with_each_roles_code_words() {
        let cache = Arc::new(GuildCache::new(Id::new(1)));
        let off = codec(
            false,
            ScanExemptions::default,
            &catalog(),
            &cache,
            Vec::new(),
        );
        assert_eq!(off.mode(), CodecMode::Passthrough);
        let zone = chrono_tz::Asia::Singapore;
        let on = codec(
            true,
            || chat_exemptions(zone, &catalog()),
            &catalog(),
            &cache,
            vec!["OtonoseKanade".into()],
        );
        assert_eq!(on.mode(), CodecMode::Pseudonymizing);
        let words = on.scan_exemptions();
        for word in [
            "Sunday",
            "September",
            "Singapore",
            "Will",
            "query",
            "OtonoseKanade",
        ] {
            assert!(words.covers(word), "chat: {word}");
        }
        for word in ["Ken", "Max", "Alice", "Sun"] {
            assert!(!words.covers(word), "chat: {word}");
        }
        let extraction = extraction_exemptions(zone, &catalog());
        for word in ["Mon", "ROSTER", "evidence_message_ids", "Will", "Hard"] {
            assert!(extraction.covers(word), "extraction: {word}");
        }
        assert!(
            !extraction.covers("September"),
            "extraction renders no month names"
        );
        assert!(!extraction.covers("Ken"));
        assert!(rewrite_exemptions().covers("playful"));
        let mut session = on.open(&[Member {
            user_id: "114200000000000011".into(),
            display_name: "Alice".into(),
            nickname: None,
            aliases: Vec::new(),
        }]);
        assert!(session.masks());
        assert_ne!(session.text("Alice"), "Alice");
    }
}
