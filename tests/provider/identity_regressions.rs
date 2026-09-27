//! Regressions from the pseudonym codec review.

use std::sync::Arc;

use kanade::infrastructure::llm::{
    governor::XorShift,
    identity::{
        BotIdentity, CodeLexicon, DecodeError, IdentityCodec, Member, NamePool, PseudonymCodec,
        PseudonymConfig, find_leaks,
    },
};
use serde_json::Value;

use super::identity::{ALICE_ID, BOB_ID, roster};

fn codec_with(pool: NamePool, seed: u64) -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool,
        lexicon: CodeLexicon::builtin(),
        bot: BotIdentity {
            user_id: Some("100000000000000001".into()),
            name: "Kanade".into(),
            aliases: Vec::new(),
        },
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(seed)),
    })
}

fn tiny() -> PseudonymCodec {
    codec_with(NamePool::from_names(["Midori", "Sumire"]), 71)
}

/// Every string (keys and values) of a JSON document.
fn strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| strings(item, out)),
        Value::Object(map) => map.iter().for_each(|(key, item)| {
            out.push(key.clone());
            strings(item, out);
        }),
        _ => {}
    }
}

#[test]
fn tool_results_mask_names_after_json_escapes() {
    let members = roster();
    let codec = codec_with(NamePool::curated(), 61);
    let mut session = codec.open(&members);
    let encoded = session.tool_result(
        r#"{"note":"carry:\nBob Lim and\tAlice Tan","who":"\u0042ob Lim","id":112233445566778899}"#,
    );
    let bob = session.member_ref(BOB_ID);
    let alice = session.member_ref(ALICE_ID);
    let value: Value = serde_json::from_str(&encoded).expect("still JSON");
    assert_eq!(value["note"], format!("carry:\n{bob} and\t{alice}"));
    assert_eq!(value["who"], bob.as_str());
    assert_eq!(value["id"], alice.as_str());
    let mut texts = Vec::new();
    strings(&value, &mut texts);
    for text in texts {
        assert_eq!(
            find_leaks(&text, &members),
            Vec::<String>::new(),
            "{text:?}"
        );
    }
    assert!(
        encoded.starts_with(r#"{"note":"carry:\n"#),
        "layout kept: {encoded}"
    );

    // Not JSON: a preceding escape is still a word boundary.
    assert_eq!(
        session.tool_result(r"carry:\nBob Lim\tAlice Tan\u0020Ali"),
        format!(r"carry:\n{bob}\t{alice}\u0020{alice}")
    );
    assert_eq!(session.text(r"x\rBob"), format!(r"x\r{bob}"));
}

#[test]
fn decode_json_sees_tokens_after_escapes() {
    let codec = tiny();
    let mut session = codec.open(&roster());
    let token = session.member_ref(ALICE_ID);
    let other = if token == "Midori" {
        "Sumire"
    } else {
        "Midori"
    };
    assert_eq!(
        session.decode_json(&format!(r#"{{"note":"x\n{token}"}}"#)),
        Ok(format!(r#"{{"note":"x\n{ALICE_ID}"}}"#))
    );
    assert_eq!(
        session.decode_json(&format!(r#"{{"note":"x\n{other}"}}"#)),
        Err(DecodeError::UnknownToken { offset: 12 })
    );
}

#[test]
fn latin_tokens_next_to_kana_are_words() {
    let codec = tiny();
    let mut session = codec.open(&roster());
    let encoded = session.text(&format!("Midoriさん and <@{ALICE_ID}>"));
    assert_eq!(session.member_ref(ALICE_ID), "Sumire", "Midori was seen");
    assert_eq!(encoded, "Midoriさん and <@Sumire>");
    assert_eq!(
        session.decode_reply("Sumireさん、Midoriさん"),
        Ok("Alice Tanさん、Midoriさん".into())
    );
    assert_eq!(session.text("Sumireちゃん"), "Midori2ちゃん");
    assert_eq!(session.decode_json("李Sumire"), Ok(format!("李{ALICE_ID}")));
    assert_eq!(
        session.decode_reply("Midori3さん"),
        Err(DecodeError::UnknownToken { offset: 0 })
    );
}

#[test]
fn late_author_named_like_a_token_is_flagged_and_kept_apart() {
    let codec = tiny();
    let mut session = codec.open(&roster());
    let x = session.member_ref(ALICE_ID);
    let y = session.author_label("700000000000000001", &x);
    assert_ne!(x, y);
    assert_eq!(
        session.text(&format!("{x} said hi")),
        format!("{y} said hi")
    );
    let needles = session.scan_needles().unwrap();
    assert!(needles.tokens.contains(&x) && needles.tokens.contains(&y));
    let clash: Vec<&str> = needles
        .names
        .iter()
        .filter(|n| n.token_clash)
        .map(|n| n.text.as_str())
        .collect();
    assert_eq!(clash, vec![x.as_str()]);
    assert!(
        !format!("{needles:?}").contains(&x),
        "Debug shows counts only"
    );
}

#[test]
fn known_ids_and_all_url_fields_are_hidden_inside_link_tokens() {
    let codec = tiny();
    let mut session = codec.open(&roster());
    let first_url = format!("https://discord.com/users/{ALICE_ID}?q=private#profile");
    let second_url = "https://x.io/path?ref=555555555555555555#section";
    let source = format!("see {first_url} and {second_url}\\nBob");
    let encoded = session.text(&source);
    let links: Vec<_> = encoded.split(' ').collect();
    assert_eq!(links[0], "see");
    assert_ne!(links[1], first_url);
    assert_ne!(links[2], second_url);
    assert_ne!(links[1], links[2]);
    assert!(!encoded.contains("https://"));
    assert!(!encoded.contains(ALICE_ID));
    assert_eq!(
        session.decode_reply(&encoded),
        Ok(format!("see {first_url} and {second_url}\\nBob"))
    );
    assert_eq!(
        session.decode_json(&format!(r#"{{"link":"{}"}}"#, links[1])),
        Ok(format!(r#"{{"link":"{first_url}"}}"#))
    );
}

#[test]
fn urls_inside_word_runs_are_still_replaced_and_restored() {
    let url = "https://private.example/path?token=fixture#fragment";
    let source = format!("prefix{url}");
    let mut session = tiny().open(&roster());
    let encoded = session.text(&source);
    assert!(!encoded.contains(url));
    assert_eq!(session.decode_reply(&encoded), Ok(source));
}

#[test]
fn escaped_urls_in_json_message_text_are_redacted_before_sending() {
    let mut session = tiny().open(&roster());
    let source = r#"{"link":"https:\/\/example.invalid/path?q=one#fragment"}"#;
    let encoded = session.text(source);
    assert!(!encoded.contains("https://"));
    assert!(!encoded.contains("example.invalid"));
    assert_eq!(
        session.decode_json(&encoded),
        Ok(r#"{"link":"https://example.invalid/path?q=one#fragment"}"#.into())
    );
}

#[test]
fn spare_tokens_seen_in_source_stay_literal() {
    let codec = codec_with(NamePool::from_names(["Kaori"]), 73);
    let roster = vec![Member {
        user_id: "500000000000000001".into(),
        display_name: "Kaori".into(),
        nickname: None,
        aliases: Vec::new(),
    }];
    let mut session = codec.open(&roster);
    assert_eq!(session.text("Nanashi1 is here"), "Nanashi1 is here");
    assert_eq!(session.member_ref("1"), "Nanashi2");
    assert_eq!(session.decode_ref("Nanashi1"), Ok("Nanashi1".into()));
    assert_eq!(session.decode_ref("nanashi2"), Ok("1".into()));
    assert_eq!(
        session.decode_ref("Nanashi3"),
        Err(DecodeError::UnknownToken { offset: 0 })
    );
    assert_eq!(session.decode_ref("Member1"), Ok("Member1".into()));
}

#[test]
fn names_right_after_a_stray_backslash_are_masked() {
    let members = roster();
    let codec = codec_with(NamePool::curated(), 79);
    let mut session = codec.open(&members);
    let bob = session.member_ref(BOB_ID);
    assert_eq!(
        session.text(r"C:\Users\bobby\x"),
        format!(r"C:\Users\{bob}\x")
    );
    assert_eq!(
        session.text(r"\nBob and \bob"),
        format!(r"\n{bob} and \{bob}")
    );
    let encoded = session.tool_result(r#"{"p":"C:\\Users\\bobby"}"#);
    assert_eq!(encoded, format!(r#"{{"p":"C:\\Users\\{bob}"}}"#));
    assert_eq!(
        session.decode_json(&encoded),
        Ok(format!(r#"{{"p":"C:\\Users\\{BOB_ID}"}}"#))
    );
}

#[test]
fn shadowing_still_sees_words_after_escapes() {
    let codec = tiny();
    let mut session = codec.open(&roster());
    let token = session.member_ref(ALICE_ID);
    let encoded = session.text(&format!(r"x\n{token}"));
    assert!(!encoded.contains(&format!(r"\n{token}")), "{encoded}");
    assert_eq!(session.decode_reply(&encoded), Ok(format!(r"x\n{token}")));
}

#[test]
fn lone_surrogate_strings_stay_valid_json() {
    let codec = codec_with(NamePool::curated(), 83);
    let mut session = codec.open(&roster());
    let bob = session.member_ref(BOB_ID);
    let encoded =
        session.tool_result(r#"{"a":"\ud800 Bob <@100000000000000001>","b":"\ud83d\ude00 bobby"}"#);
    let value: Value = serde_json::from_str(&encoded).expect("still JSON");
    assert_eq!(value["a"], format!("\u{fffd} {bob} @Kanade"));
    assert_eq!(value["b"], format!("😀 {bob}"));
    // Unchanged strings keep their bytes, lone surrogate included.
    let untouched = r#"{"c":"\udc00 hi","d":["\ud800\ud800"]}"#;
    assert_eq!(session.tool_result(untouched), untouched);
}
