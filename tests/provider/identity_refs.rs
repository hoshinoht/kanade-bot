//! Wire-ports codec additions: extraction message refs, opaque refs for stray
//! snowflakes (text, URLs, JSON), channel and role names, the issued-name
//! mapping and code-owned text kept literal by `encode_protected`.

use std::sync::Arc;

use kanade::infrastructure::llm::governor::XorShift;
use kanade::infrastructure::llm::identity::{
    BotIdentity, CodeLexicon, DecodeError, IdentityCodec, IdentitySession, Member, MentionNames,
    NamePool, Passthrough, Protected, PseudonymCodec, PseudonymConfig, encode_protected,
};

const ALICE: &str = "114200000000000011";
const STRAY: &str = "123456789012345678";

fn codec() -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity::default(),
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(3)),
    })
}

fn roster() -> Vec<Member> {
    vec![
        Member {
            user_id: ALICE.into(),
            display_name: "Alice".into(),
            nickname: None,
            aliases: Vec::new(),
        },
        Member {
            user_id: "114200000000000012".into(),
            display_name: "Will".into(),
            nickname: None,
            aliases: Vec::new(),
        },
    ]
}

#[test]
fn message_refs_are_short_per_session_and_decode_back() {
    let mut session = codec().open(&roster());
    assert!(session.masks());
    assert_eq!(session.message_ref("1142000000000009001"), "1");
    assert_eq!(session.message_ref("1142000000000009002"), "2");
    assert_eq!(session.message_ref("1142000000000009001"), "1");
    assert_eq!(
        session.decode_message_ref("2"),
        Ok("1142000000000009002".into())
    );
    assert_eq!(
        session.decode_message_ref(" [1] "),
        Ok("1142000000000009001".into())
    );
    for unknown in ["3", "0", "x", ""] {
        assert!(matches!(
            session.decode_message_ref(unknown),
            Err(DecodeError::UnknownToken { .. })
        ));
    }
    let mut plain = Passthrough.open(&roster());
    assert!(!plain.masks());
    assert_eq!(plain.message_ref("99"), "99");
    assert_eq!(plain.decode_message_ref("anything"), Ok("anything".into()));
    assert_eq!(plain.mapping(), None);
}

#[test]
fn stray_snowflakes_become_opaque_refs_everywhere_and_decode() {
    let mut session = codec().open(&roster());
    let text = session.text(&format!("{STRAY} https://discord.com/x/{STRAY} <#{STRAY}>"));
    assert!(!text.contains(STRAY), "{text}");
    assert_eq!(text, "Ref1 https://discord.com/x/Ref1 <#Ref1>");
    let json = session.tool_result(&format!(r#"{{"id": {STRAY}, "n": 5}}"#));
    assert_eq!(json, r#"{"id": "Ref1", "n": 5}"#);
    assert_eq!(
        session.decode_reply("see <#Ref1> and ref1"),
        Ok(format!("see <#{STRAY}> and {STRAY}"))
    );
    // A `Ref<n>` spelled by a member stays literal and is never issued.
    let mut session = codec().open(&roster());
    let own = session.text(&format!("Ref1 then {STRAY}"));
    assert_eq!(own, "Ref1 then Ref2");
    assert_eq!(session.decode_reply("Ref2"), Ok(STRAY.to_owned()));
}

struct Names;

impl MentionNames for Names {
    fn channel(&self, id: &str) -> Option<String> {
        (id == "900").then(|| "alice-runs".to_owned())
    }

    fn role(&self, id: &str) -> Option<String> {
        (id == "901").then(|| "Raiders".to_owned())
    }
}

#[test]
fn channel_and_role_mentions_render_as_names_with_members_masked() {
    let codec = codec().with_mention_names(Arc::new(Names));
    let mut session = codec.open(&roster());
    let alice = session.member_ref(ALICE);
    let text = session.text("in <#900> ping <@&901> or <#902>");
    assert_eq!(text, format!("in #{alice}-runs ping @Raiders or <#902>"));
}

#[test]
fn the_mapping_lists_issued_member_tokens_only() {
    let mut session = codec().open_session(&roster());
    let alice = session.member_ref(ALICE);
    let stranger = session.author_label("114200000000000099", "Zed");
    session.text(STRAY);
    let mapping = session.mapping().expect("masking");
    assert_eq!(mapping.len(), 2);
    assert_eq!(mapping[0].token, alice);
    assert_eq!(mapping[0].user_id, ALICE);
    assert_eq!(mapping[0].display_name.as_deref(), Some("Alice"));
    assert_eq!(mapping[1].token, stranger);
    assert_eq!(mapping[1].display_name.as_deref(), Some("Zed"));
    let shown = format!("{mapping:?} {session:?}");
    for private in ["Alice", "Zed", ALICE, alice.as_str()] {
        assert!(!shown.contains(private), "{shown}");
    }
}

#[test]
fn protected_code_text_stays_literal_while_member_text_is_masked() {
    let mut session = codec().open(&roster());
    let rules = "You will read the chat.";
    let text = format!("Will asked. {rules} Alice too.");
    let encoded = encode_protected(session.as_mut(), &text, &[Protected::Exact(rules.into())]);
    assert!(encoded.contains(rules), "{encoded}");
    assert!(!encoded.starts_with("Will"), "{encoded}");
    assert!(!encoded.contains("Alice"));
    assert_eq!(
        encode_protected(
            &mut *Passthrough.open(&roster()),
            &text,
            &[Protected::Exact(rules.into())]
        ),
        text
    );
}

#[test]
fn former_names_are_masked_and_a_departed_member_shows_the_latest() {
    let mut session = codec().open(&roster());
    session.former_name(ALICE, "Oldnick");
    session.former_name("114200000000000077", "Early Ghost");
    session.former_name("114200000000000077", "Late Ghost");
    let text = session.text("Oldnick and Late Ghost and Early Ghost");
    for old in ["Oldnick", "Ghost"] {
        assert!(!text.contains(old), "{text}");
    }
    let ghost = session.member_ref("114200000000000077");
    assert_eq!(session.decode_reply(&ghost), Ok("Late Ghost".to_owned()));
    let alice = session.member_ref(ALICE);
    assert_eq!(
        session.decode_reply(&alice),
        Ok("Alice".to_owned()),
        "roster name wins"
    );
    let names = session.scan_needles().unwrap().names;
    assert!(names.iter().any(|name| name.text == "Oldnick"), "scanned");
}

fn word_roster() -> Vec<Member> {
    vec![
        Member {
            user_id: "114200000000000031".into(),
            display_name: "Jonas lau".into(),
            nickname: None,
            aliases: Vec::new(),
        },
        Member {
            user_id: "114200000000000032".into(),
            display_name: "hoshi".into(),
            nickname: Some("Will Smith".into()),
            aliases: vec!["Ken tan".into()],
        },
    ]
}

struct PartyNames;

impl MentionNames for PartyNames {
    fn channel(&self, id: &str) -> Option<String> {
        (id == "900").then(|| "hbaldguy-jonas-cryz-hoshi".to_owned())
    }

    fn role(&self, _: &str) -> Option<String> {
        None
    }
}

/// The live shape: party channels named after members' first names.
#[test]
fn words_of_multi_word_names_are_masked_in_channel_names_and_tool_results() {
    let codec = codec().with_mention_names(Arc::new(PartyNames));
    let mut session = codec.open(&word_roster());
    let jonas = session.member_ref("114200000000000031");
    let hoshi = session.member_ref("114200000000000032");
    let text = session.text("runs in <#900>: jonas, LAU and hoshi");
    assert_eq!(
        text,
        format!("runs in #hbaldguy-{jonas}-cryz-{hoshi}: {jonas}, {jonas} and {hoshi}")
    );
    let result = session
        .tool_result(r##"{"runs": [{"channel": "#hstar-jonas_lau-Yoshi", "who": "Jonas lau"}]}"##);
    for raw in ["jonas", "Jonas", "lau"] {
        assert!(!result.contains(raw), "{raw}: {result}");
    }
    assert_eq!(
        session.decode_reply(&format!("{jonas} is on it")),
        Ok("Jonas lau is on it".to_owned())
    );
    // Longest match still wins: the full name is one token, not two.
    assert_eq!(session.text("Jonas lau"), jonas);
    // Stopwords and lexicon words are not word needles (full names still are).
    assert_eq!(session.text("tan will go"), "tan will go");
    assert_eq!(session.text("ask smith"), format!("ask {hoshi}"));
    assert_eq!(session.text("Ken tan"), hoshi);
    assert_eq!(session.text("Will Smith"), hoshi);
    // The scanner knows the words too.
    let needles = session.scan_needles().unwrap();
    for word in ["jonas", "lau", "Smith"] {
        assert!(
            needles
                .names
                .iter()
                .any(|name| name.text.eq_ignore_ascii_case(word)),
            "{word}"
        );
    }
    for word in ["tan", "Will", "Ken"] {
        assert!(
            !needles.names.iter().any(|name| name.text == word),
            "{word}"
        );
    }
}

fn person(user_id: &str, name: &str) -> Member {
    Member {
        user_id: user_id.into(),
        display_name: name.into(),
        nickname: None,
        aliases: Vec::new(),
    }
}

const ALEX_TAN: &str = "114200000000000041";
const ALEX: &str = "114200000000000042";
const KEVIN: &str = "114200000000000043";
const SARAH: &str = "114200000000000044";

/// A whole name outranks another member's name word, so `Alex` is Alex,
/// never `Alex Tan`, however the roster is ordered.
#[test]
fn a_whole_name_beats_another_members_word() {
    for roster in [
        vec![person(ALEX_TAN, "Alex Tan"), person(ALEX, "Alex")],
        vec![person(ALEX, "Alex"), person(ALEX_TAN, "Alex Tan")],
    ] {
        let mut session = codec().open(&roster);
        let label = session.author_label(ALEX, "Alex");
        let full = session.member_ref(ALEX_TAN);
        assert_ne!(label, full);
        assert_eq!(
            session.text("is alex coming? #hstar-alex"),
            format!("is {label} coming? #hstar-{label}")
        );
        assert_eq!(session.text("Alex Tan"), full);
        assert_eq!(session.decode_ref(&label), Ok(ALEX.to_owned()));
        assert_eq!(session.decode_reply(&label), Ok("Alex".to_owned()));
        let issued = session.participant_enum().unwrap();
        assert!(issued.contains(&label) && issued.contains(&full));
    }
}

/// A word two members share (`Lim`) maps to neither: masked and scanned,
/// but no participant and never decoded to a member.
#[test]
fn a_shared_surname_is_masked_but_belongs_to_nobody() {
    let mut session = codec().open(&[person(KEVIN, "Kevin Lim"), person(SARAH, "Sarah Lim")]);
    let kevin = session.member_ref(KEVIN);
    let sarah = session.member_ref(SARAH);
    let text = session.text("ask lim and kevin in #hstar-lim");
    assert!(!text.to_lowercase().contains("lim"), "{text}");
    let shared = text.split(' ').nth(1).unwrap().to_owned();
    assert!(shared != kevin && shared != sarah, "{text}");
    assert_eq!(text, format!("ask {shared} and {kevin} in #hstar-{shared}"));
    assert!(matches!(
        session.decode_ref(&shared),
        Err(DecodeError::UnknownToken { .. })
    ));
    assert!(session.decode_json(&format!(r#"["{shared}"]"#)).is_err());
    assert_eq!(
        session.decode_reply(&format!("{shared} is in")),
        Ok("Lim is in".to_owned())
    );
    assert!(!session.participant_enum().unwrap().contains(&shared));
    assert!(
        !session
            .mapping()
            .unwrap()
            .iter()
            .any(|name| name.token == shared)
    );
    let needles = session.scan_needles().unwrap();
    assert!(
        needles.names.iter().any(|name| name.text == "Lim"),
        "scanned"
    );
}

/// A word that becomes shared mid-session maps to nobody from then on;
/// what was encoded before keeps its member.
#[test]
fn a_word_shared_later_is_ambiguous_from_then_on() {
    let mut session = codec().open(&[person(KEVIN, "Kevin Lim")]);
    let kevin = session.text("lim");
    assert_eq!(session.decode_ref(&kevin), Ok(KEVIN.to_owned()));
    session.author_label(SARAH, "Sarah Lim");
    let later = session.text("lim");
    assert_ne!(later, kevin);
    assert!(session.decode_ref(&later).is_err());
    assert_eq!(
        session.decode_ref(&kevin),
        Ok(KEVIN.to_owned()),
        "never retroactive"
    );
}

/// A member's name word never masks the bot's or the persona's own name.
#[test]
fn bot_and_persona_names_are_never_word_needles() {
    let codec = PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity {
            user_id: Some("114200000000000001".into()),
            name: "Kanade".into(),
            aliases: vec!["Kanata".into()],
        },
        extra_exclusions: vec!["OtonoseKanade".into(), "Otonose".into()],
        random: Arc::new(XorShift::new(3)),
    });
    let mut session = codec.open(&[
        person("114200000000000051", "Kanade Fanclub"),
        person("114200000000000052", "Otonose Mori"),
    ]);
    let text = session.text("You are Kanade (Kanata), Otonose; fanclub mori");
    assert!(
        text.starts_with("You are Kanade (Kanata), Otonose; "),
        "{text}"
    );
    assert!(
        !text.contains("fanclub") && !text.contains("mori"),
        "{text}"
    );
}
