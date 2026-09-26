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
