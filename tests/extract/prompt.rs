use chrono::{NaiveTime, Utc};
use kanade::domain::attendance::AttendanceDefault;
use kanade::domain::catalog::BossTable;
use kanade::domain::schedule::{FixedRun, Run};
use kanade::domain::weeks;
use kanade::extract::prompt::{
    self, PromptContext, PromptMessage, SYSTEM_PROMPT, build_messages, extraction_request,
};
use kanade::extract::schema::{
    AttemptOutcome, ExtractionAttempts, Next, extraction_schema, schema_text,
};
use kanade::infrastructure::llm::identity::{
    IdentityCodec, Member, PassthroughSession, TaggingCodec, find_request_leaks,
};
use serde_json::{Value, json};

use crate::shaping;
use crate::support::{
    Deviation, Outcome, body, capabilities, catalog, instant, messages, messages_json, reasoning,
    replay_family_with, runs, text, unknown_op, zone,
};

fn prompt_message(raw: &Value) -> PromptMessage {
    PromptMessage {
        id: text(&raw["id"]).to_owned(),
        author_id: text(&raw["author_id"]).to_owned(),
        author_name: text(&raw["author_name"]).to_owned(),
        created_at: instant(&raw["created_at"]).with_timezone(&Utc),
        content: text(&raw["content"]).to_owned(),
    }
}

fn fixed_run(raw: &Value) -> FixedRun {
    let weekday = raw["weekday"].as_i64().expect("weekday");
    FixedRun {
        id: text(&raw["id"]).to_owned(),
        owner_id: String::new(),
        channel_id: raw["channel_id"].as_str().map(str::to_owned),
        bosses: crate::support::strings(&raw["bosses"]),
        weekday: weeks::weekday_from_index(weekday).expect("0..=6"),
        time: text(&raw["time"]).parse::<NaiveTime>().expect("HH:MM"),
        participants: crate::support::strings(&raw["participants"]),
        note: None,
        attendance_default: AttendanceDefault::default(),
        standing: Vec::new(),
    }
}

fn member(raw: &Value) -> Member {
    Member {
        user_id: text(&raw["user_id"]).to_owned(),
        display_name: text(&raw["display_name"]).to_owned(),
        nickname: raw["nickname"].as_str().map(str::to_owned),
        aliases: Vec::new(),
    }
}

/// Owned storage a `PromptContext` borrows from.
struct Owned {
    burst: Vec<PromptMessage>,
    context: Vec<PromptMessage>,
    runs: Vec<Run>,
    guild_runs: Vec<Run>,
    fixed: Vec<FixedRun>,
    roster: Vec<Member>,
    channel: String,
}

impl Owned {
    fn from_json(raw: &Value) -> Self {
        let list = |key: &str| raw[key].as_array().expect("list").clone();
        Self {
            burst: list("burst").iter().map(prompt_message).collect(),
            context: list("context").iter().map(prompt_message).collect(),
            runs: runs(&raw["runs"]),
            guild_runs: runs(&raw["guild_runs"]),
            fixed: list("fixed_runs").iter().map(fixed_run).collect(),
            roster: list("roster").iter().map(member).collect(),
            channel: text(&raw["channel_name"]).to_owned(),
        }
    }

    fn with<R>(
        &self,
        zone: chrono_tz::Tz,
        table: &BossTable,
        f: impl FnOnce(&PromptContext<'_>) -> R,
    ) -> R {
        let runs: Vec<&Run> = self.runs.iter().collect();
        let guild_runs: Vec<&Run> = self.guild_runs.iter().collect();
        f(&PromptContext {
            zone,
            table,
            burst: &self.burst,
            context: &self.context,
            runs: &runs,
            fixed_runs: &self.fixed,
            roster: &self.roster,
            channel_name: &self.channel,
            guild_runs: &guild_runs,
        })
    }
}

fn replay(input: &Value, step: &Value) -> Outcome {
    let zone = zone(input);
    let table = catalog(&input["catalog"]);
    let with_context = |f: &dyn Fn(&PromptContext<'_>) -> Value| {
        Owned::from_json(&step["context"]).with(zone, &table, |context| f(context))
    };
    let value = match text(&step["op"]) {
        "system_prompt" => json!(SYSTEM_PROMPT),
        "build_messages" => with_context(&|context| {
            messages_json(&build_messages(context, &mut PassthroughSession))
        }),
        "relevant_roster" => with_context(&|context| {
            json!(
                prompt::relevant_roster(context)
                    .iter()
                    .map(|member| member.user_id.as_str())
                    .collect::<Vec<_>>()
            )
        }),
        "named_bosses" => with_context(&|context| json!(prompt::named_bosses(context))),
        "json_schema" => json!({ "schema": extraction_schema(None), "text": schema_text() }),
        "json_instruction" => json!(shaping::v5_instruction()),
        "extraction_body" => {
            let request = extraction_request(
                text(&step["model"]),
                messages(&step["messages"]),
                reasoning(&step["reasoning_effort"]),
                &PassthroughSession,
            );
            body(&request, &capabilities(&step["caps"]))
        }
        "estimate_tokens" => json!(prompt::estimate_tokens(text(&step["text"]))),
        "estimate_messages" => json!(prompt::estimate_messages(&messages(&step["messages"]))),
        "prompt_budget" => {
            let num_ctx =
                usize::try_from(step["num_ctx"].as_u64().expect("num_ctx")).expect("usize");
            json!(prompt::prompt_budget(num_ctx))
        }
        "json_instruction_tokens" => {
            json!(prompt::schema_instruction_tokens(&extraction_schema(None)))
        }
        other => unknown_op("prompt", other),
    };
    Ok(value)
}

const BODY_CASE: &str = "structured-output-schema-and-request-body";

const DEVIATIONS: [Deviation; 6] = [
    Deviation {
        name: "D-SHAPING runner schema instruction",
        case_id: BODY_CASE,
        step: 1,
        rewrite: shaping::instruction,
    },
    Deviation {
        name: "D-SHAPING sampled structured body",
        case_id: BODY_CASE,
        step: 2,
        rewrite: shaping::body,
    },
    Deviation {
        name: "D-SHAPING unpublished effort refused",
        case_id: BODY_CASE,
        step: 3,
        rewrite: shaping::refused_effort,
    },
    Deviation {
        name: "D-SHAPING schema in prompt",
        case_id: BODY_CASE,
        step: 4,
        rewrite: shaping::body,
    },
    Deviation {
        name: "D-SHAPING schema in prompt, sampled",
        case_id: BODY_CASE,
        step: 5,
        rewrite: shaping::body,
    },
    Deviation {
        name: "D-SHAPING runner instruction budget",
        case_id: "token-budget-estimates",
        step: 6,
        rewrite: shaping::instruction_tokens,
    },
];

#[test]
fn prompt_vectors_replay_exactly() {
    let replayed = replay_family_with("prompt", &DEVIATIONS, |_, _| {}, replay);
    assert_eq!(replayed, (4, 23));
}

fn tagged_fixture() -> (Vec<Member>, Owned) {
    let roster = vec![
        Member {
            user_id: "114200000000000011".into(),
            display_name: "Alvin tan".into(),
            nickname: None,
            aliases: vec!["alv".into()],
        },
        Member {
            user_id: "114200000000000022".into(),
            display_name: "kanon [AZUR]".into(),
            nickname: Some("kanon".into()),
            aliases: Vec::new(),
        },
        Member {
            user_id: "114200000000000033".into(),
            display_name: "Priya".into(),
            nickname: None,
            aliases: Vec::new(),
        },
    ];
    let at = |text: &str| instant(&json!(text)).with_timezone(&Utc);
    let message = |id: &str, author: &Member, when: &str, content: &str| PromptMessage {
        id: id.into(),
        author_id: author.user_id.clone(),
        author_name: author
            .nickname
            .clone()
            .unwrap_or_else(|| author.display_name.clone()),
        created_at: at(when),
        content: content.into(),
    };
    let run = |id: &str, participants: &[&Member]| Run {
        id: id.into(),
        fixed_run_id: None,
        channel_id: Some("900".into()),
        week_start: at("2026-08-27T00:00:00+08:00"),
        datetime: at("2026-08-31T21:30:00+08:00"),
        bosses: vec!["HMaleficStar".into()],
        participants: participants.iter().map(|m| m.user_id.clone()).collect(),
        status: kanade::domain::schedule::RunStatus::Planned,
        source: kanade::domain::schedule::RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    };
    let owned = Owned {
        burst: vec![
            message(
                "401",
                &roster[0],
                "2026-08-30T13:01:00+08:00",
                "kanon can u do wed? <@114200000000000022>",
            ),
            message(
                "402",
                &roster[1],
                "2026-08-30T13:05:00+08:00",
                "ok for wed, Alvin tan",
            ),
        ],
        context: vec![message(
            "398",
            &roster[2],
            "2026-08-30T12:40:00+08:00",
            "alv free mon?",
        )],
        runs: vec![run(
            "a1a1a1a1-0000-4000-8000-000000000001",
            &[&roster[0], &roster[1]],
        )],
        guild_runs: Vec::new(),
        fixed: vec![FixedRun {
            participants: vec![roster[2].user_id.clone()],
            ..fixed_run(&json!({
                "id": "f6f6f6f6-0000-4000-8000-000000000006",
                "bosses": ["HLimbo"],
                "weekday": 1,
                "time": "22:30",
                "participants": [],
                "channel_id": "900",
            }))
        }],
        roster: roster.clone(),
        channel: "hstar-kanon".into(),
    };
    (roster, owned)
}

fn fixture_table() -> BossTable {
    let file = crate::support::load("prompt.json");
    catalog(&file["cases"][0]["input"]["catalog"])
}

#[test]
fn tagging_codec_keeps_every_roster_identity_out_of_the_request() {
    let (roster, owned) = tagged_fixture();
    let table = fixture_table();
    let zone = chrono_tz::Asia::Kuala_Lumpur;

    let plain = owned.with(zone, &table, |context| {
        extraction_request(
            "extractor",
            build_messages(context, &mut PassthroughSession),
            None,
            &PassthroughSession,
        )
    });
    assert!(
        !find_request_leaks(&plain, &roster).is_empty(),
        "the fixture must expose identities without a codec"
    );

    let mut session = TaggingCodec.open(&roster);
    let request = owned.with(zone, &table, |context| {
        let messages = build_messages(context, session.as_mut());
        extraction_request("extractor", messages, None, session.as_ref())
    });
    assert_eq!(find_request_leaks(&request, &roster), Vec::<String>::new());
    let schema = &request.output_schema.as_ref().expect("schema").schema;
    let refs = &schema["$defs"]["Amendment"]["properties"]["participants"]["items"]["enum"];
    assert_eq!(
        *refs,
        json!(session.participant_enum().expect("tagging refs"))
    );

    // The model answers in refs; the call decodes them back to user ids.
    let tag = session.member_ref(&roster[1].user_id);
    let reply = format!(
        r#"{{"amendments":[{{"kind":"rsvp","participants":["{tag}"],"rsvp":"yes","evidence_message_ids":["402"]}}],"summary":"{tag} agrees"}}"#
    );
    let mut attempts = ExtractionAttempts::new(request.messages.clone());
    let outcome = AttemptOutcome::Reply {
        content: Some(reply),
        reasoning: None,
    };
    let Next::Done(call) = attempts.record(outcome, session.as_ref()) else {
        panic!("a valid answer ends the call");
    };
    let extraction = call.extraction.expect("accepted");
    assert_eq!(
        extraction.amendments[0].participants,
        [roster[1].user_id.clone()]
    );
    assert_eq!(extraction.summary, format!("{} agrees", roster[1].user_id));

    // A ref the session never issued is a malformed answer: retried, not trusted.
    let mut attempts = ExtractionAttempts::new(request.messages);
    let unknown = AttemptOutcome::Reply {
        content: Some(r#"{"amendments":[{"kind":"rsvp","participants":["ID99"]}]}"#.into()),
        reasoning: None,
    };
    assert_eq!(attempts.record(unknown, session.as_ref()), Next::Retry);
}
