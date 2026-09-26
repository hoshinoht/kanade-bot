use kanade::infrastructure::llm::{
    ChatRequest, CompletionResponse, FakeAction, FinishReason, Message, OutputSchema,
    OutputValidation, ToolCall, ToolCallRequest, ToolDefinition,
    governor::{Role, RoleRoute},
    identity::{
        CodecMode, DecodeError, IdentityCodec, IdentitySession, Member, Passthrough, RouteRefused,
        TaggingCodec, check_routes, find_request_leaks, guard, open_session, unmasked,
    },
};
use serde_json::json;

use super::support::build_runner;

pub(super) const ALICE_ID: &str = "112233445566778899";
pub(super) const BOB_ID: &str = "998877665544332211";
pub(super) const OUTSIDER_ID: &str = "123456789012345678";

pub(super) fn roster() -> Vec<Member> {
    vec![
        Member {
            user_id: ALICE_ID.into(),
            display_name: "Alice Tan".into(),
            nickname: Some("Ali".into()),
            aliases: vec!["alicebishop".into()],
        },
        Member {
            user_id: BOB_ID.into(),
            display_name: "Bob".into(),
            nickname: None,
            aliases: vec!["bobby".into(), "Bob Lim".into()],
        },
    ]
}

struct Line<'a> {
    author_id: &'a str,
    author_name: &'a str,
    content: &'a str,
}

const BURST: [Line<'static>; 3] = [
    Line {
        author_id: ALICE_ID,
        author_name: "Alice Tan",
        content: "mon cannot, <@998877665544332211> can change to wed?",
    },
    Line {
        author_id: BOB_ID,
        author_name: "Bob",
        content: "ok for wed, ask bobby's friend Ali and <@!123456789012345678>",
    },
    Line {
        author_id: ALICE_ID,
        author_name: "Alice Tan",
        content: "Bob Lim confirm? alicebishop out, 112233445566778899 is me",
    },
];

/// A port-shaped prompt; `None` renders exactly as a port without a codec would.
pub(super) fn build_request(mut session: Option<&mut dyn IdentitySession>) -> ChatRequest {
    let mut lines = vec!["RUNS: #a1 HMaleficStar Mon 21:30".to_owned()];
    let participants: Vec<String> = match session.as_deref_mut() {
        Some(s) => s.participants(&[ALICE_ID, BOB_ID]),
        None => vec![ALICE_ID.into(), BOB_ID.into()],
    };
    lines.push(format!("participants: {}", participants.join(", ")));
    for (index, line) in BURST.iter().enumerate() {
        let (label, mention, content) = match session.as_deref_mut() {
            Some(s) => (
                s.author_label(line.author_id, line.author_name),
                s.mention(line.author_id),
                s.text(line.content),
            ),
            None => (
                line.author_name.to_owned(),
                format!("<@{}>", line.author_id),
                line.content.to_owned(),
            ),
        };
        lines.push(format!("[{}] [{label} {mention}] {content}", index + 1));
    }
    let tool_result = format!(r#"{{"member":"{BOB_ID}","name":"Bob"}}"#);
    let tool_result = match session.as_deref_mut() {
        Some(s) => s.tool_result(&tool_result),
        None => tool_result,
    };
    let participant_enum = session.as_deref().and_then(|s| s.participant_enum());
    let mut participant = json!({"type": "string"});
    if let Some(values) = participant_enum {
        participant["enum"] = json!(values);
    }
    ChatRequest {
        model: "m".into(),
        messages: vec![
            Message::System {
                content: "extract".into(),
            },
            Message::User {
                content: lines.join("\n"),
            },
            Message::Assistant {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: "call_1".into(),
                    name: "read_member".into(),
                    arguments: "{}".into(),
                }],
            },
            Message::Tool {
                tool_call_id: "call_1".into(),
                content: tool_result,
            },
        ],
        tools: vec![ToolDefinition {
            name: "read_member".into(),
            description: None,
            input_schema: json!({"type": "object"}),
        }],
        output_schema: Some(OutputSchema {
            name: "amendments".into(),
            schema: json!({
                "type": "object",
                "properties": {"participants": {"type": "array", "items": participant}},
                "required": ["participants"],
                "additionalProperties": false
            }),
            strict: true,
            validation: OutputValidation::Runner,
        }),
        max_output_tokens: 256,
        reasoning: None,
        sampling: None,
    }
}

fn reply(content: &str) -> CompletionResponse {
    CompletionResponse {
        model: "m".into(),
        content: Some(content.into()),
        tool_calls: Vec::new(),
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

/// The request exactly as the provider received it, its JSON, and the reply.
pub(super) async fn capture(
    request: &ChatRequest,
    answer: &str,
) -> (ChatRequest, String, CompletionResponse) {
    let (provider, runner) = build_runner([FakeAction::Response(reply(answer))]);
    let response = runner.complete(request).await.expect("fake completion");
    let mut sent = provider.requests();
    assert_eq!(sent.len(), 1);
    let sent = sent.remove(0);
    let wire = serde_json::to_string(&sent).unwrap();
    (sent, wire, response)
}

pub(super) fn route(external: bool) -> RoleRoute {
    RoleRoute {
        role: Role::Chat,
        alias: "cloud-chat".into(),
        group: Some("cloud".into()),
        external,
        unmasked_allowed: false,
        effort: None,
    }
}

#[tokio::test]
async fn passthrough_request_is_byte_identical_to_no_codec() {
    let bare = build_request(None);
    let mut session = Passthrough.open(&roster());
    let coded = build_request(Some(session.as_mut()));
    assert!(bare == coded);

    let answer = format!(r#"{{"participants":["{ALICE_ID}","<@{BOB_ID}>"]}}"#);
    let (_, bare_wire, bare_response) = capture(&bare, &answer).await;
    let (_, coded_wire, coded_response) = capture(&coded, &answer).await;
    assert_eq!(bare_wire, coded_wire);
    assert!(bare_response == coded_response);
    let content = coded_response.content.unwrap();
    assert_eq!(session.decode_json(&content).unwrap(), content);
}

#[test]
fn passthrough_round_trips_generated_inputs_byte_for_byte() {
    const PIECES: &[&str] = &[
        "Alice Tan",
        "ali",
        "<@112233445566778899>",
        "<@!998877665544332211>",
        "123456789012345678",
        "ID1",
        "P2's",
        "{\"participants\":[\"11\"]}",
        "\\u0000",
        "\"",
        " ",
        "\n",
        "é",
        "漢字",
        "🙂",
        "\u{200b}",
        "",
    ];
    let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let members = roster();
    for _ in 0..500 {
        let len = (next() % 12) as usize;
        let input: String = (0..len)
            .map(|_| PIECES[(next() % PIECES.len() as u64) as usize])
            .collect();
        let mut session = Passthrough.open(&members);
        assert_eq!(session.text(&input), input);
        assert_eq!(session.tool_result(&input), input);
        assert_eq!(session.author_label(ALICE_ID, &input), input);
        assert_eq!(session.member_ref(&input), input);
        assert_eq!(session.mention(&input), format!("<@{input}>"));
        assert_eq!(session.decode_ref(&input).unwrap(), input);
        assert_eq!(session.decode_json(&input).unwrap(), input);
        assert_eq!(session.decode_reply(&input).unwrap(), input);
        assert_eq!(session.participant_enum(), None);
    }
}

#[test]
fn passthrough_reports_passthrough_mode() {
    assert_eq!(Passthrough.mode(), CodecMode::Passthrough);
    assert_eq!(TaggingCodec.mode(), CodecMode::Pseudonymizing);
}

#[test]
fn external_route_fails_closed_while_passthrough() {
    let refused = RouteRefused::ExternalWithoutPseudonymization {
        role: Role::Chat,
        alias: "cloud-chat".into(),
    };
    assert_eq!(guard(&route(true), &Passthrough), Err(refused.clone()));
    assert_eq!(
        open_session(&Passthrough, &route(true), &roster()).err(),
        Some(refused.clone())
    );
    assert_eq!(
        check_routes(&[route(false), route(true)], &Passthrough),
        Err(refused)
    );
}

#[test]
fn local_routes_pass_and_pseudonymizing_codec_unlocks_external() {
    assert_eq!(guard(&route(false), &Passthrough), Ok(()));
    assert!(open_session(&Passthrough, &route(false), &roster()).is_ok());
    assert_eq!(check_routes(&[route(false)], &Passthrough), Ok(()));
    assert_eq!(guard(&route(true), &TaggingCodec), Ok(()));
    assert!(open_session(&TaggingCodec, &route(true), &roster()).is_ok());
}

#[test]
fn the_operator_override_lets_external_routes_through_unmasked() {
    let allowed = RoleRoute {
        unmasked_allowed: true,
        ..route(true)
    };
    assert_eq!(guard(&allowed, &Passthrough), Ok(()));
    assert!(open_session(&Passthrough, &allowed, &roster()).is_ok());
    assert_eq!(
        check_routes(std::slice::from_ref(&allowed), &Passthrough),
        Ok(())
    );
    assert!(unmasked(&allowed, &Passthrough));
    assert!(!unmasked(&allowed, &TaggingCodec));
    assert!(!unmasked(&route(false), &Passthrough));
    assert!(guard(&route(true), &Passthrough).is_err(), "off by default");
}

#[tokio::test]
async fn tagging_capture_has_no_raw_identity_and_passthrough_capture_does() {
    let members = roster();
    let mut session = open_session(&TaggingCodec, &route(true), &members).unwrap();
    let tagged = build_request(Some(session.as_mut()));
    let (sent, _, _) = capture(&tagged, r#"{"participants":["ID1"]}"#).await;
    assert_eq!(find_request_leaks(&sent, &members), Vec::<String>::new());
    let Message::User { content } = &sent.messages[1] else {
        panic!("user prompt expected");
    };
    assert!(content.contains("[3] [ID1 <@ID1>] ID2 confirm? ID1 out, ID1 is me"));
    assert!(content.contains("ask ID2's friend ID1 and <@ID3>"));
    assert_eq!(
        session.participant_enum(),
        Some(vec!["ID1".into(), "ID2".into(), "ID3".into()])
    );

    let (plain, _, _) = capture(&build_request(None), r#"{"participants":[]}"#).await;
    let leaks = find_request_leaks(&plain, &members);
    for raw in [
        ALICE_ID,
        BOB_ID,
        OUTSIDER_ID,
        "Alice Tan",
        "Ali",
        "bobby",
        "Bob Lim",
        "alicebishop",
    ] {
        assert!(leaks.iter().any(|leak| leak == raw), "scanner missed {raw}");
    }
}

#[tokio::test]
async fn tagging_decodes_tool_arguments_and_replies() {
    let members = roster();
    let mut session = TaggingCodec.open(&members);
    build_request(Some(session.as_mut()));
    let (provider, runner) = build_runner([FakeAction::Response(CompletionResponse {
        model: "m".into(),
        content: None,
        tool_calls: vec![ToolCall {
            id: "call_2".into(),
            name: "read_member".into(),
            arguments: r#"{"participant":"id2"}"#.into(),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    })]);
    let mut request = build_request(None);
    request.output_schema = None;
    let response = runner.complete(&request).await.unwrap();
    assert_eq!(provider.requests().len(), 1);
    assert_eq!(
        session.decode_json(&response.tool_calls[0].arguments),
        Ok(format!(r#"{{"participant":"{BOB_ID}"}}"#))
    );
    assert_eq!(session.decode_ref("ID1"), Ok(ALICE_ID.to_owned()));
    assert_eq!(
        session.decode_reply("ID2's run moved; ID1 and ID3 confirm"),
        Ok(format!(
            "Bob's run moved; Alice Tan and {OUTSIDER_ID} confirm"
        ))
    );
    assert_eq!(
        session.decode_reply("IDEA id 5ID1"),
        Ok("IDEA id 5ID1".into())
    );
    assert_eq!(
        session.decode_json(r#"{"participant":"ID9"}"#),
        Err(DecodeError::UnknownToken { offset: 16 })
    );
    assert_eq!(
        session.decode_ref("ID99999999999999999999999"),
        Err(DecodeError::UnknownToken { offset: 0 })
    );
}

#[test]
fn tagging_sessions_do_not_share_state() {
    let members = roster();
    let mut first = TaggingCodec.open(&members);
    assert_eq!(first.member_ref(BOB_ID), "ID1");
    let second = TaggingCodec.open(&members);
    assert_eq!(second.participant_enum(), Some(Vec::new()));
    assert_eq!(
        second.decode_ref("ID1"),
        Err(DecodeError::UnknownToken { offset: 0 })
    );
}
