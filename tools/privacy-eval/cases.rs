use kanade::infrastructure::llm::{
    Message, ToolCallRequest,
    identity::{IdentityGrant, Member},
};
use serde::Deserialize;
use serde_json::json;

const FIXTURE: &str = include_str!("../../tests/fixtures/privacy-evaluation.json");

pub struct FixtureSet {
    pub cases: Vec<EvaluationCase>,
    pub boss_alias_collision: String,
}

pub struct EvaluationCase {
    label: &'static str,
    roster: Vec<Member>,
    messages: Vec<SourceMessage>,
    candidates: Vec<String>,
    ambiguous_terms: Vec<String>,
}

impl EvaluationCase {
    pub fn label(&self) -> &'static str {
        self.label
    }

    pub fn roster(&self) -> &[Member] {
        &self.roster
    }

    pub fn candidates(&self) -> &[String] {
        &self.candidates
    }

    pub fn ambiguous_terms(&self) -> &[String] {
        &self.ambiguous_terms
    }

    pub fn encode_messages(&self, grant: &mut IdentityGrant) -> Vec<Message> {
        self.messages
            .iter()
            .map(|source| match source {
                SourceMessage::User(content) => Message::User {
                    content: grant.text(content),
                },
                SourceMessage::ToolCall(arguments) => Message::Assistant {
                    content: None,
                    tool_calls: vec![ToolCallRequest {
                        id: "privacy-probe-call-1".into(),
                        name: "propose_add".into(),
                        arguments: grant.text(arguments),
                    }],
                },
                SourceMessage::ToolResult(content) => Message::Tool {
                    tool_call_id: "privacy-probe-call-1".into(),
                    content: grant.tool_result(content),
                },
            })
            .collect()
    }
}

enum SourceMessage {
    User(String),
    ToolCall(String),
    ToolResult(String),
}

#[derive(Deserialize)]
struct Fixture {
    chat_members: Vec<SyntheticMember>,
    extraction_members: Vec<SyntheticMember>,
    target_id: String,
    boss_alias_collision: String,
    chat: ChatFixture,
    extraction: ExtractionFixture,
}

#[derive(Deserialize)]
struct SyntheticMember {
    user_id: String,
    display_name: String,
    nickname: Option<String>,
    aliases: Vec<String>,
}

#[derive(Deserialize)]
struct ChatFixture {
    question: String,
    name_in_url: String,
    non_roster_url: String,
    answer: String,
}

#[derive(Deserialize)]
struct ExtractionFixture {
    target_id: String,
    message_id: String,
    content: String,
    name_in_url: String,
    non_roster_url: String,
    summary: String,
}

pub fn load() -> Result<FixtureSet, ()> {
    let fixture: Fixture = serde_json::from_str(FIXTURE).map_err(|_| ())?;
    let chat_candidates = identity_strings(&fixture.chat_members, &fixture.boss_alias_collision);
    let extraction_candidates =
        identity_strings(&fixture.extraction_members, &fixture.boss_alias_collision);
    let ambiguous_terms = vec![fixture.boss_alias_collision.clone()];
    let chat_members = members(fixture.chat_members);
    let extraction_members = members(fixture.extraction_members);
    let cases = vec![
        EvaluationCase {
            label: "chat_history_tool_results",
            roster: chat_members,
            messages: vec![
                SourceMessage::User(fixture.chat.question.clone()),
                SourceMessage::ToolCall(
                    json!({
                        "boss": fixture.boss_alias_collision.clone(),
                        "when": "wed 9pm",
                        "participants": format!("<@{}>", fixture.target_id),
                    })
                    .to_string(),
                ),
                SourceMessage::ToolResult(format!("Draft card: {}", fixture.chat.answer)),
            ],
            candidates: chat_candidates,
            ambiguous_terms: ambiguous_terms.clone(),
        },
        EvaluationCase {
            label: "extraction_burst",
            roster: extraction_members.clone(),
            messages: vec![
                SourceMessage::User(format!(
                    "Burst item 1. Author id: {}. Message id: {}. {}",
                    fixture.extraction.target_id,
                    fixture.extraction.message_id,
                    fixture.extraction.content
                )),
                SourceMessage::User(format!("Burst item 2. {}", fixture.extraction.name_in_url)),
                SourceMessage::User(format!(
                    "Burst item 3. {}",
                    fixture.extraction.non_roster_url
                )),
                SourceMessage::User(format!("Burst summary: {}", fixture.extraction.summary)),
            ],
            candidates: extraction_candidates.clone(),
            ambiguous_terms: ambiguous_terms.clone(),
        },
        EvaluationCase {
            label: "card_rewrite_context",
            roster: extraction_members.clone(),
            messages: vec![SourceMessage::User(format!(
                "Draft party card: {} Channel: #hstar-VESPER-quill. Attendee id: {}",
                fixture.chat.answer, fixture.extraction.target_id
            ))],
            candidates: extraction_candidates.clone(),
            ambiguous_terms: ambiguous_terms.clone(),
        },
        EvaluationCase {
            label: "full_url_masking",
            roster: extraction_members,
            messages: vec![
                SourceMessage::User(fixture.chat.name_in_url),
                SourceMessage::User(fixture.chat.non_roster_url),
                SourceMessage::User(fixture.extraction.name_in_url),
                SourceMessage::User(fixture.extraction.non_roster_url),
            ],
            candidates: extraction_candidates,
            ambiguous_terms,
        },
    ];
    Ok(FixtureSet {
        cases,
        boss_alias_collision: fixture.boss_alias_collision,
    })
}

fn members(raw: Vec<SyntheticMember>) -> Vec<Member> {
    raw.into_iter()
        .map(|member| Member {
            user_id: member.user_id,
            display_name: member.display_name,
            nickname: member.nickname,
            aliases: member.aliases,
        })
        .collect()
}

fn identity_strings(members: &[SyntheticMember], ambiguous_term: &str) -> Vec<String> {
    members
        .iter()
        .flat_map(|member| {
            std::iter::once(member.user_id.clone())
                .chain(std::iter::once(member.display_name.clone()))
                .chain(member.nickname.iter().cloned())
                .chain(member.aliases.iter().cloned())
        })
        .filter(|candidate| !candidate.eq_ignore_ascii_case(ambiguous_term))
        .collect()
}
