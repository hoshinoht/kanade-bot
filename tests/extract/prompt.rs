use chrono::{DateTime, NaiveTime, Weekday};
use kanade_domain::attendance::AttendanceDefault;
use kanade_domain::catalog::BossTable;
use kanade_domain::schedule::{FixedRun, Run};
use kanade_extract::prompt::{PromptContext, PromptMessage, build_messages, extraction_request};
use kanade_extract::schema::{AttemptOutcome, ExtractionAttempts, Next};
use kanade_llm::identity::{Member, PassthroughSession};

use crate::support::fixture_catalog;

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

fn raw_fixture() -> (Vec<Member>, Owned) {
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
    let at = |text: &str| DateTime::parse_from_rfc3339(text).unwrap().to_utc();
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
        status: kanade_domain::schedule::RunStatus::Planned,
        source: kanade_domain::schedule::RunSource::Amend,
        attendance: Vec::new(),
        status_pin: None,
    };
    let owned = Owned {
        burst: vec![
            message(
                MSG_401,
                &roster[0],
                "2026-08-30T13:01:00+08:00",
                "kanon can u do wed? <@114200000000000022> https://synthetic.invalid/member?id=fixture",
            ),
            message(
                MSG_402,
                &roster[1],
                "2026-08-30T13:05:00+08:00",
                "ok for wed, Alvin tan",
            ),
        ],
        context: vec![message(
            MSG_398,
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
            owner_pinned: false,
            id: "f6f6f6f6-0000-4000-8000-000000000006".into(),
            owner_id: String::new(),
            channel_id: Some("900".into()),
            bosses: vec!["HLimbo".into()],
            weekday: Weekday::Tue,
            time: NaiveTime::from_hms_opt(22, 30, 0).unwrap(),
            participants: vec![roster[2].user_id.clone()],
            note: None,
            attendance_default: AttendanceDefault::default(),
            standing: Vec::new(),
        }],
        roster: roster.clone(),
        channel: "hstar-kanon".into(),
    };
    (roster, owned)
}

const MSG_398: &str = "1419000000000000398";
const MSG_401: &str = "1419000000000000401";
const MSG_402: &str = "1419000000000000402";

#[test]
fn extraction_prompts_send_raw_member_data_and_ids() {
    let (roster, owned) = raw_fixture();
    let table = fixture_catalog();
    let zone = chrono_tz::Asia::Kuala_Lumpur;
    let mut session = PassthroughSession;
    let request = owned.with(zone, &table, |context| {
        let messages = build_messages(context, &mut session);
        extraction_request("extractor", messages, None)
    });
    let sent = serde_json::to_string(&request.messages).unwrap();
    for raw in [
        "Alvin tan",
        "kanon",
        "114200000000000011",
        MSG_401,
        "https://synthetic.invalid/member?id=fixture",
    ] {
        assert!(sent.contains(raw), "{raw} missing from {sent}");
    }
    let schema = &request.output_schema.as_ref().expect("schema").schema;
    assert!(
        schema["$defs"]["Amendment"]["properties"]["participants"]["items"]
            .get("enum")
            .is_none()
    );

    let reply = format!(
        r#"{{"amendments":[{{"kind":"rsvp","participants":["{}"],"rsvp":"yes","evidence_message_ids":["{MSG_402}"]}}],"summary":"SyntheticMember agrees"}}"#,
        roster[1].user_id
    );
    let mut attempts = ExtractionAttempts::new(request.messages.clone());
    let outcome = AttemptOutcome::Reply {
        content: Some(reply),
        reasoning: None,
    };
    let Next::Done(call) = attempts.record(outcome) else {
        panic!("a valid answer ends the call");
    };
    let extraction = call.extraction.expect("accepted");
    assert_eq!(
        extraction.amendments[0].participants,
        [roster[1].user_id.clone()]
    );
    assert_eq!(extraction.summary, "SyntheticMember agrees");
}
