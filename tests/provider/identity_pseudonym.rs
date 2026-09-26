use std::{collections::BTreeSet, sync::Arc};

use kanade::infrastructure::llm::{
    Message,
    governor::XorShift,
    identity::{
        BotIdentity, CodeLexicon, CodecMode, DecodeError, IdentityCodec, IdentitySession, Member,
        NamePool, Passthrough, PseudonymCodec, PseudonymConfig, SystemRng, find_leaks,
        find_request_leaks, open_session,
    },
};

use super::identity::{
    ALICE_ID, BOB_ID, OUTSIDER_ID, build_request, capture, roster as burst_roster, route,
};

const BOT_ID: &str = "100000000000000001";
const SUN_ID: &str = "300000000000000001";
const WILL_ID: &str = "300000000000000002";
const STRANGER_ID: &str = "300000000000000009";

fn bot() -> BotIdentity {
    BotIdentity {
        user_id: Some(BOT_ID.into()),
        name: "Kanade".into(),
        aliases: vec!["Kanata".into(), "Yuuki".into(), "Sakuna".into()],
    }
}

fn codec_with(pool: NamePool, seed: u64) -> PseudonymCodec {
    PseudonymCodec::new(PseudonymConfig {
        pool,
        lexicon: CodeLexicon::builtin(),
        bot: bot(),
        extra_exclusions: Vec::new(),
        random: Arc::new(XorShift::new(seed)),
    })
}

fn codec(seed: u64) -> PseudonymCodec {
    codec_with(NamePool::curated(), seed)
}

fn member(user_id: &str, display_name: &str, aliases: &[&str]) -> Member {
    Member {
        user_id: user_id.into(),
        display_name: display_name.into(),
        nickname: None,
        aliases: aliases.iter().map(|&a| a.into()).collect(),
    }
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Whole-word, ASCII case-insensitive occurrence.
fn has_word(haystack: &str, word: &str) -> bool {
    let lower = haystack.to_lowercase();
    let word = word.to_lowercase();
    lower.match_indices(&word).any(|(at, _)| {
        lower[..at].chars().next_back().is_none_or(|c| !is_word(c))
            && lower[at + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !is_word(c))
    })
}

/// `text` with every `http(s)://` URL removed (URLs end at whitespace,
/// `<>"` + backtick or a backslash), then escapes turned into spaces so the
/// leak finder sees word boundaries.
fn without_urls(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if tail.starts_with("https://") || tail.starts_with("http://") {
            let end = tail
                .find(|c: char| c.is_whitespace() || "<>\"`\\".contains(c))
                .unwrap_or(tail.len());
            out.push(' ');
            rest = &tail[end..];
        } else {
            out.push_str("http");
            rest = &tail[4..];
        }
    }
    out.push_str(rest);
    out.replace("\\n", "  ").replace("\\t", "  ")
}

#[test]
fn reports_pseudonymizing_and_passthrough_exposes_no_needles() {
    assert_eq!(codec(1).mode(), CodecMode::Pseudonymizing);
    assert!(Passthrough.open(&burst_roster()).scan_needles().is_none());
}

#[test]
fn issues_lazily_and_enum_is_exactly_the_issued_member_tokens() {
    let codec = codec(3);
    let mut session = codec.open(&burst_roster());
    assert_eq!(session.participant_enum(), Some(Vec::new()));
    assert_eq!(session.text("mon 21:30 HStar p1?"), "mon 21:30 HStar p1?");
    assert_eq!(session.participant_enum(), Some(Vec::new()));
    let bob = session.member_ref(BOB_ID);
    assert_eq!(session.participant_enum(), Some(vec![bob.clone()]));
    let alice = session.author_label(ALICE_ID, "Alice Tan");
    assert_ne!(alice, bob);
    assert_eq!(session.member_ref(BOB_ID), bob, "stable within a session");
    assert_eq!(session.mention(ALICE_ID), format!("<@{alice}>"));
    assert_eq!(session.participant_enum(), Some(vec![bob, alice]));
}

#[test]
fn sessions_are_unlinkable() {
    let codec = codec(11);
    let roster = burst_roster();
    let mut pairs = BTreeSet::new();
    for _ in 0..200 {
        let mut session = codec.open(&roster);
        pairs.insert((session.member_ref(ALICE_ID), session.member_ref(BOB_ID)));
    }
    assert!(
        pairs.len() >= 195,
        "only {} distinct assignments",
        pairs.len()
    );

    let system = PseudonymCodec::new(PseudonymConfig {
        pool: NamePool::curated(),
        lexicon: CodeLexicon::builtin(),
        bot: bot(),
        extra_exclusions: Vec::new(),
        random: Arc::new(SystemRng::new()),
    });
    let alice: BTreeSet<String> = (0..20)
        .map(|_| system.open(&roster).member_ref(ALICE_ID))
        .collect();
    assert!(alice.len() > 1, "system randomness varies tokens");
}

#[test]
fn round_trips_text_json_and_refs() {
    let codec = codec(5);
    let mut session = codec.open(&burst_roster());
    let encoded = session.text("Alice Tan, bobby's friend Ali and <@!998877665544332211> ok");
    let alice = session.member_ref(ALICE_ID);
    let bob = session.member_ref(BOB_ID);
    assert_eq!(
        encoded,
        format!("{alice}, {bob}'s friend {alice} and <@{bob}> ok")
    );
    assert_eq!(
        session.decode_json(&format!(r#"{{"participants":["{alice}","<@{bob}>"]}}"#)),
        Ok(format!(
            r#"{{"participants":["{ALICE_ID}","<@{BOB_ID}>"]}}"#
        ))
    );
    assert_eq!(session.decode_ref(&bob), Ok(BOB_ID.to_owned()));
    assert_eq!(
        session.decode_reply(&encoded),
        Ok("Alice Tan, Bob's friend Alice Tan and Bob ok".to_owned())
    );
}

#[test]
fn decoding_is_possessive_and_case_safe() {
    let codec = codec(8);
    let mut session = codec.open(&burst_roster());
    let t = session.member_ref(BOB_ID);
    let lower = t.to_lowercase();
    let upper = t.to_uppercase();
    let reply = format!("{t}, {lower}, {upper}! {t}'s {t}s' {t}’s <@{t}> <@!{t}> @{t}");
    assert_eq!(
        session.decode_reply(&reply),
        Ok("Bob, Bob, Bob! Bob's Bobs' Bob’s Bob Bob @Bob".to_owned())
    );
    assert_eq!(
        session.decode_json(&format!(r#"["{lower}","{t}'s"]"#)),
        Ok(format!(r#"["{BOB_ID}","{BOB_ID}'s"]"#))
    );
}

#[test]
fn longest_match_and_word_boundaries() {
    let codec = codec(9);
    let mut session = codec.open(&burst_roster());
    let encoded = session.text(
        "Bob Lim, bobby, (Bob), Bob's; Bobcat BobLim ali-baba Alice Tanner https://x.io/Bob?u=Bob",
    );
    let alice = session.member_ref(ALICE_ID);
    let bob = session.member_ref(BOB_ID);
    assert_eq!(
        encoded,
        format!(
            "{bob}, {bob}, ({bob}), {bob}'s; Bobcat BobLim {alice}-baba Alice Tanner https://x.io/Bob?u=Bob"
        )
    );
}

#[test]
fn unicode_and_decorated_names() {
    let roster = vec![
        member("400000000000000001", "Ｒｙｏ", &[]),
        member("400000000000000002", "Zoë", &["李"]),
        member("400000000000000003", "李小龍", &[]),
        member("400000000000000004", "🌸Mika🌸", &[]),
    ];
    let codec = codec(13);
    let mut session = codec.open(&roster);
    let encoded = session.text("ｒｙｏ & ZOË met 我李小龍们 with 🌸Mika🌸 (aka Mika) at 李's");
    let [ryo, zoe, li, mika] =
        ["1", "2", "3", "4"].map(|n| session.member_ref(&format!("40000000000000000{n}")));
    assert_eq!(
        encoded,
        format!("{ryo} & {zoe} met 我{li}们 with {mika} (aka {mika}) at 李's")
    );
    let needles = session.scan_needles().unwrap();
    assert_eq!(needles.skipped_short, 1, "李 is too short to mask");
    assert_eq!(
        session.decode_reply(&format!("{zoe} and {mika}")),
        Ok("Zoë and 🌸Mika🌸".to_owned())
    );
}

#[test]
fn members_named_like_code_words_are_masked_and_reported() {
    let roster = vec![
        member(SUN_ID, "Sun", &[]),
        member(WILL_ID, "Will", &["willow"]),
    ];
    let codec = codec(17);
    let mut session = codec.open(&roster);
    let encoded = session.text("Sun and Will clear Lucid on Mon; sun works, willow too, Sunday no");
    let sun = session.member_ref(SUN_ID);
    let will = session.member_ref(WILL_ID);
    assert_eq!(
        encoded,
        format!("{sun} and {will} clear Lucid on Mon; {sun} works, {will} too, Sunday no")
    );
    let needles = session.scan_needles().unwrap();
    assert_eq!(needles.collisions(), 2);
    let colliding: BTreeSet<&str> = needles
        .names
        .iter()
        .filter(|n| n.collides)
        .map(|n| n.text.as_str())
        .collect();
    assert_eq!(colliding, BTreeSet::from(["Sun", "Will"]));
    assert!(needles.ids.contains(&SUN_ID.to_owned()));
}

#[test]
fn unknown_authors_and_ids_are_masked_in_later_text() {
    let codec = codec(19);
    let mut session = codec.open(&burst_roster());
    let label = session.author_label(OUTSIDER_ID, "Zed Quill");
    let stranger = session.member_ref(STRANGER_ID);
    let encoded = session.text(&format!(
        "Zed Quill asked zed quill? {OUTSIDER_ID} <@{OUTSIDER_ID}> x{STRANGER_ID} {STRANGER_ID}."
    ));
    assert_eq!(
        encoded,
        format!("{label} asked {label}? {label} <@{label}> x{stranger} {stranger}.")
    );
    for raw in ["Zed", "Quill", OUTSIDER_ID, STRANGER_ID] {
        assert!(!encoded.contains(raw), "{raw} leaked");
    }
    assert_eq!(
        session.decode_reply(&format!("{label} and {stranger}")),
        Ok("Zed Quill and someone".to_owned())
    );
    assert_eq!(session.decode_ref(&stranger), Ok(STRANGER_ID.to_owned()));
    // Stray snowflakes of nobody in the session are left for message refs.
    assert_eq!(session.text("555555555555555555"), "555555555555555555");
}

#[test]
fn bot_is_never_tokenised() {
    let mut roster = burst_roster();
    roster.push(member(BOT_ID, "Kanade", &["kana-bot"]));
    let codec = codec(23);
    let mut session = codec.open(&roster);
    assert_eq!(session.author_label(BOT_ID, "OtonoseKanade"), "Kanade");
    assert_eq!(session.mention(BOT_ID), "@Kanade");
    assert_eq!(session.member_ref(BOT_ID), "Kanade");
    assert_eq!(
        session.text(&format!("<@{BOT_ID}> hi Kanade {BOT_ID}")),
        "@Kanade hi Kanade Kanade"
    );
    assert_eq!(session.participant_enum(), Some(Vec::new()));
    assert_eq!(session.decode_ref("Kanade"), Ok(BOT_ID.to_owned()));
    assert_eq!(
        session.decode_reply("Kanade says hi"),
        Ok("Kanade says hi".into())
    );
}

#[test]
fn unknown_tokens_are_quarantined() {
    let codec = codec_with(NamePool::from_names(["Midori", "Sumire", "Kaori"]), 29);
    let mut session = codec.open(&burst_roster());
    let issued = session.member_ref(ALICE_ID);
    let unissued = ["Midori", "Sumire", "Kaori"]
        .into_iter()
        .find(|name| *name != issued)
        .unwrap();
    assert_eq!(
        session.decode_json(&format!(r#"{{"p":"{unissued}"}}"#)),
        Err(DecodeError::UnknownToken { offset: 6 })
    );
    assert_eq!(
        session.decode_reply(&format!("ok {unissued}'s")),
        Err(DecodeError::UnknownToken { offset: 3 })
    );
    assert_eq!(
        session.decode_ref(&format!("{unissued}2")),
        Err(DecodeError::UnknownToken { offset: 0 })
    );
    assert_eq!(
        session.decode_reply(&format!("<@{unissued}>")),
        Err(DecodeError::UnknownToken { offset: 2 })
    );
    assert_eq!(
        session.decode_reply("Haruka Mira Midoriko"),
        Ok("Haruka Mira Midoriko".into())
    );
}

#[test]
fn pool_names_in_source_stay_literal() {
    let codec = codec_with(NamePool::from_names(["Midori", "Sumire"]), 31);
    let mut session = codec.open(&burst_roster());
    let encoded = session.text(&format!("Sumire's cat saw <@{ALICE_ID}>"));
    assert_eq!(
        session.member_ref(ALICE_ID),
        "Midori",
        "Sumire is never issued"
    );
    assert_eq!(encoded, "Sumire's cat saw <@Midori>");
    assert_eq!(
        session.decode_reply("Sumire told Midori"),
        Ok("Sumire told Alice Tan".into())
    );
    assert_eq!(session.decode_ref("sumire"), Ok("sumire".into()));

    // A word spelling an already issued token is shadowed, not confused.
    let encoded = session.text("midori is a colour; Midoris too");
    assert_eq!(encoded, "Midori2 is a colour; Midori2s too");
    assert_eq!(
        session.decode_reply(&encoded),
        Ok("midori is a colour; midoris too".into())
    );
    assert_eq!(session.decode_json("Midori"), Ok(ALICE_ID.into()));
    assert_eq!(session.participant_enum(), Some(vec!["Midori".to_owned()]));
}

#[test]
fn exhausted_pool_falls_back_to_numbered_names() {
    let codec = codec_with(NamePool::from_names(["Midori", "Sumire", "Kaori"]), 37);
    let roster = vec![member("500000000000000001", "Kaori", &[])];
    let mut session = codec.open(&roster);
    let ids: Vec<String> = (0..6).map(|n| format!("6{n:017}")).collect();
    let tokens: Vec<String> = ids.iter().map(|id| session.member_ref(id)).collect();
    let first: BTreeSet<&str> = tokens[..2].iter().map(String::as_str).collect();
    assert_eq!(
        first,
        BTreeSet::from(["Midori", "Sumire"]),
        "Kaori is a member"
    );
    assert_eq!(tokens[2..], ["Midori2", "Sumire2", "Midori3", "Sumire3"]);
    for (id, token) in ids.iter().zip(&tokens) {
        assert_eq!(session.decode_ref(token).as_ref(), Ok(id));
        assert_eq!(
            session.decode_ref(&format!("{token}'s")),
            Ok(format!("{id}'s"))
        );
    }
    assert_eq!(
        session.decode_ref("Midori4"),
        Err(DecodeError::UnknownToken { offset: 0 })
    );

    let empty = codec_with(NamePool::from_names(["Kaori"]), 41);
    let mut session = empty.open(&roster);
    assert_eq!(session.member_ref("1"), "Nanashi1");
    assert_eq!(session.member_ref("2"), "Nanashi2");
    assert_eq!(session.decode_ref("nanashi2"), Ok("2".into()));
    assert_eq!(
        session.decode_ref("Nanashi3"),
        Err(DecodeError::UnknownToken { offset: 0 })
    );
}

#[test]
fn debug_output_has_no_names_or_ids() {
    let roster = vec![
        member(SUN_ID, "Sun", &[]),
        member(WILL_ID, "Will", &["Zed Quill"]),
    ];
    let codec = codec(43);
    let mut session = codec.open_session(&roster);
    let token = session.author_label(OUTSIDER_ID, "Stranger Danger");
    session.text("Sun and Will");
    let dumps = [
        format!("{codec:?}"),
        format!("{session:?}"),
        format!("{:?}", session.scan_needles().unwrap()),
        format!("{:?}", bot()),
        format!("{:?}", NamePool::curated()),
        format!("{:?}", CodeLexicon::builtin()),
    ];
    for dump in dumps {
        for raw in [
            "Sun",
            "Will",
            "Zed",
            "Stranger",
            "Kanade",
            "Kanata",
            SUN_ID,
            WILL_ID,
            OUTSIDER_ID,
            BOT_ID,
            token.as_str(),
        ] {
            assert!(!dump.contains(raw), "{dump} leaks {raw}");
        }
    }
}

#[tokio::test]
async fn capture_has_no_raw_identity_and_decodes_back() {
    let members = burst_roster();
    let codec = codec(47);
    let mut session = open_session(&codec, &route(true), &members).unwrap();
    let request = build_request(Some(session.as_mut()));
    let enum_values = session.participant_enum().unwrap();
    assert_eq!(enum_values.len(), 3, "Alice, Bob and the outsider");
    let answer = format!(r#"{{"participants":["{}"]}}"#, enum_values[1]);
    let (sent, _, response) = capture(&request, &answer).await;
    assert_eq!(find_request_leaks(&sent, &members), Vec::<String>::new());
    let Message::User { content } = &sent.messages[1] else {
        panic!("user prompt expected");
    };
    assert!(!has_word(content, OUTSIDER_ID));
    assert_eq!(
        session.decode_json(&response.content.unwrap()),
        Ok(format!(r#"{{"participants":["{BOB_ID}"]}}"#))
    );
}

#[test]
fn generated_text_round_trips_and_never_leaks() {
    const PIECES: &[&str] = &[
        "<@112233445566778899>",
        "<@998877665544332211>",
        "<@123456789012345678>",
        "112233445566778899",
        "998877665544332211",
        "Midori",
        "sumire's",
        "Kaori2",
        "HStar",
        "p2",
        "https://x.io/Bob",
        "{\"participants\":[\"a\"]}",
        "é",
        "漢字",
        "🙂",
        "\u{200b}",
        "ok",
        "",
    ];
    const NAMES: &[&str] = &["Alice Tan", "ali", "BOBBY", "Bob Lim", "alicebishop", "bob"];
    const SEPARATORS: &[&str] = &[" ", ", ", "\n", " (", ") ", "'s ", "\\n", "\\t", "\\"];
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let members = burst_roster();
    let pool = NamePool::from_names(["Midori", "Sumire", "Kaori", "Tomoe", "Wakaba"]);
    let codec = codec_with(pool, 53);
    for _ in 0..500 {
        let mut plain = Vec::new();
        let mut named = Vec::new();
        for _ in 0..(next() % 10) {
            let sep = SEPARATORS[(next() % SEPARATORS.len() as u64) as usize];
            let piece = PIECES[(next() % PIECES.len() as u64) as usize];
            plain.push(format!("{piece}{sep}"));
            let name = NAMES[(next() % NAMES.len() as u64) as usize];
            named.push(format!("{piece}{sep}{name}{sep}"));
        }
        let plain: String = plain.concat();
        let named: String = named.concat();
        let mut session = codec.open(&members);
        let encoded = session.text(&plain);
        assert_eq!(session.decode_json(&encoded).as_deref(), Ok(plain.as_str()));
        let encoded = session.tool_result(&named);
        let leaks = find_leaks(&without_urls(&encoded), &members);
        assert!(leaks.is_empty(), "{named:?} -> {encoded:?} leaks {leaks:?}");
        assert!(session.decode_reply(&encoded).is_ok());
    }
}
