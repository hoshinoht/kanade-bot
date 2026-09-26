use std::{collections::BTreeSet, path::Path, sync::Arc};

use kanade::infrastructure::{
    files::load_catalog,
    llm::{
        governor::XorShift,
        identity::{
            BotIdentity, CodeLexicon, IdentityCodec, Member, NamePool, PseudonymCodec,
            PseudonymConfig,
        },
    },
};

const PERSONAS: [&str; 4] = ["Kanade", "Kanata", "Yuuki", "Sakuna"];

fn bot() -> BotIdentity {
    BotIdentity {
        user_id: Some("100000000000000001".into()),
        name: "Kanade".into(),
        aliases: PERSONAS[1..].iter().map(|&s| s.into()).collect(),
    }
}

fn catalog_lexicon() -> CodeLexicon {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml");
    CodeLexicon::from_boss_table(&load_catalog(&path).expect("tracked catalog loads"))
}

fn codec(pool: NamePool, lexicon: CodeLexicon, seed: u64) -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool,
        lexicon,
        bot: bot(),
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(seed)),
    })
}

fn curated_lines() -> Vec<&'static str> {
    include_str!("../../src/infrastructure/llm/identity/pseudonym/names.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

#[test]
fn curated_list_is_well_formed() {
    let lines = curated_lines();
    let pool = NamePool::curated();
    assert!(pool.len() >= 250, "pool too small: {}", pool.len());
    assert_eq!(
        pool.len(),
        lines.len(),
        "every line is a usable, unique name"
    );
    for name in pool.names() {
        assert!(name.len() >= 3 && name.bytes().all(|b| b.is_ascii_alphabetic()));
        assert!(name.starts_with(|c: char| c.is_ascii_uppercase()), "{name}");
        assert!(!name.ends_with('s'), "{name}");
    }
}

#[test]
fn curated_names_survive_the_static_filters() {
    let lexicon = CodeLexicon::builtin();
    let pool = NamePool::curated();
    let rejected: Vec<&str> = pool.names().filter(|name| lexicon.excludes(name)).collect();
    assert_eq!(rejected, Vec::<&str>::new(), "lexicon or stopword clash");
    let near_bot: Vec<&str> = pool
        .names()
        .filter(|name| codec(NamePool::from_names([*name]), lexicon.clone(), 1).pool_size() == 0)
        .collect();
    assert_eq!(near_bot, Vec::<&str>::new(), "near a bot or persona name");
}

#[test]
fn curated_names_mostly_survive_the_tracked_catalog() {
    let lexicon = catalog_lexicon();
    let pool = NamePool::curated();
    let rejected: Vec<&str> = pool.names().filter(|name| lexicon.excludes(name)).collect();
    assert_eq!(rejected, Vec::<&str>::new());
}

#[test]
fn stopwords_and_code_words_never_become_pool_names() {
    let lexicon = CodeLexicon::builtin();
    for word in [
        "Ran", "Mai", "Rin", "Ken", "Sun", "Sunday", "Will", "Lucid", "Kanna", "Hayato", "Ren",
        "Lara", "Kain", "Khali", "Akechi", "Hard", "Chaos", "Jun", "Taro", "Hero", "Hiro",
    ] {
        assert!(lexicon.excludes(word), "{word} must be excluded");
    }
    let pool = NamePool::from_names(["Ran", "Mai", "Rin", "Sunny", "Hiro", "Yuki", "Midori"]);
    let codec = codec(pool, CodeLexicon::builtin(), 7);
    let mut session = codec.open(&[]);
    let first = session.member_ref("1");
    assert_eq!(
        first, "Midori",
        "only Midori survives (Yuki is near persona Yuuki)"
    );
}

#[test]
fn drawn_tokens_avoid_roster_near_matches_lexicon_and_personas() {
    let roster = vec![
        Member {
            user_id: "200000000000000001".into(),
            display_name: "Sayaka☆".into(),
            nickname: Some("kyouko".into()),
            aliases: vec!["Hinata99".into(), "Takumii".into()],
        },
        Member {
            user_id: "200000000000000002".into(),
            display_name: "Shiori Tan".into(),
            nickname: None,
            aliases: vec!["Mizue".into()],
        },
    ];
    let lexicon = catalog_lexicon();
    let codec = codec(NamePool::curated(), lexicon.clone(), 42);
    let mut session = codec.open(&roster);
    let pool = NamePool::curated().len();
    let tokens: Vec<String> = (0..pool + 20)
        .map(|n| session.member_ref(&format!("3{n:017}")))
        .collect();
    let unique: BTreeSet<&String> = tokens.iter().collect();
    assert_eq!(unique.len(), tokens.len());
    let banned = [
        "sayaka", "kyoko", "hinata", "takumi", "takuma", "shiori", "mizue", "kanade", "kanata",
        "yuki", "sakuna",
    ];
    for token in &tokens {
        let base = token.trim_end_matches(|c: char| c.is_ascii_digit());
        assert!(!lexicon.excludes(base), "{token} is a code word");
        let lower = base.to_ascii_lowercase();
        assert!(!banned.contains(&lower.as_str()), "{token} is too close");
    }
    assert!(tokens.iter().any(|t| t.ends_with('2')), "fallback used");
}
