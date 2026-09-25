//! C3 traffic and safety around a question: allowance overrides, refunds
//! and the once-per-episode limited reply; the per-channel queue; the
//! clean-retry storm guard; code-only routing; the post-answer glue with
//! pollution containment; failure lines; and the Limits view.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use kanade::chat::answer::{AnswerDeps, AnswerFailure, Generation, Question, answer};
use kanade::chat::context::{QuestionMessage, Reference, WITHHELD, build_turns};
use kanade::chat::gate::{
    Author, ChannelInfo, IncomingMessage, PilotSettings, RATE_LIMITED, Summons, decide,
};
use kanade::chat::pilot::{
    Admission, CONTENT_BLOCKED_REPLY, ChatPilot, Finished, GuardLimits, LogFacts, ReplyPort,
    TrafficLimits,
};
use kanade::chat::sanitize::FAILURE_REPLY;
use kanade::chat::tools::bundles::{Bundle, CardContext, ToolOffer};
use kanade::chat::tools::{ToolContext, ToolName};
use kanade::domain::model_log::{AllowanceOverride, ChatOutcome};
use kanade::infrastructure::llm::governor::{Charge, Refused, SessionError, SessionFailure};
use kanade::infrastructure::llm::identity::Passthrough;
use kanade::infrastructure::llm::{
    CompletionResponse, FakeAction, FakeProvider, FinishReason, Message,
};
use serde_json::json;

use crate::looping::{Ports, roster, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::load;
use crate::wire::kanade;
use crate::world::{Channels, World, pilot};

const BOT: &str = "5000";
const MODEL: &str = "synthetic-chat";

/// Records replies; each gets id `reply-<n>`.
#[derive(Default)]
struct Replies(Mutex<Vec<(String, String, String)>>);

impl ReplyPort for Replies {
    async fn post_reply(
        &self,
        channel_id: &str,
        reply_to: &str,
        text: &str,
    ) -> Result<String, String> {
        let mut posted = self.0.lock().unwrap();
        posted.push((channel_id.into(), reply_to.into(), text.into()));
        Ok(format!("reply-{}", posted.len()))
    }
}

fn gate_input() -> (Channels, PilotSettings) {
    let input = &load("gate.json")["cases"][0]["input"];
    (Channels::new(&input["channels"]), pilot(&input["settings"]))
}

fn asked_by(member: &str) -> IncomingMessage {
    IncomingMessage {
        author: Some(Author {
            id: member.into(),
            bot: false,
            roles: vec!["6000".into()],
        }),
        guild_id: Some("1000".into()),
        channel: Some(ChannelInfo::bare("700")),
        mentions: vec![BOT.into()],
        role_mentions: Vec::new(),
    }
}

fn summons() -> Summons<'static> {
    Summons {
        bot_user_id: Some(BOT),
        self_role_id: None,
        replied_author_id: None,
        enabled: true,
        is_admin: false,
    }
}

fn ctx(member: &str) -> ToolContext {
    ToolContext::new(
        member,
        "700",
        "8100",
        Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap(),
    )
}

fn facts(id: &str) -> LogFacts<'static> {
    LogFacts {
        id: id.into(),
        at: Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap(),
        model: MODEL,
        reasoning: None,
        latency_ms: 10,
    }
}

fn new_pilot() -> ChatPilot {
    ChatPilot::new(2700.0, TrafficLimits::default(), GuardLimits::default())
}

#[test]
fn overrides_refunds_and_one_limited_reply_per_episode() {
    let (channels, settings) = gate_input();
    let mut pilot = new_pilot();
    let override_row = AllowanceOverride {
        member_id: "11".into(),
        count: 1,
        window_ms: 60_000,
        updated_at: Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap(),
    };
    pilot
        .allowance
        .apply((4, 300.0), (12, 900.0), &[override_row]);
    let first = decide(
        &asked_by("11"),
        &settings,
        &channels,
        summons(),
        pilot.allowance.budgets(10.0),
    );
    assert!(first.act);
    let second = decide(
        &asked_by("11"),
        &settings,
        &channels,
        summons(),
        pilot.allowance.budgets(20.0),
    );
    assert_eq!((second.reason, second.retry_after_s), (RATE_LIMITED, 50.0));

    let (reply, row) = pilot.limited(&ctx("11"), "again?", &second, facts("limited-1"), 20.0);
    assert_eq!(
        reply.as_deref(),
        Some("That's your 1 answer for now — ask me again in about 50s."),
        "their own override, not the default"
    );
    assert_eq!(row.outcome, ChatOutcome::RateLimited);
    let (again, row) = pilot.limited(&ctx("11"), "again?", &second, facts("limited-2"), 25.0);
    assert_eq!(again, None, "said once per episode");
    assert_eq!(
        (row.outcome, row.reply.as_str()),
        (ChatOutcome::RateLimited, "")
    );

    // A refunded question gives its slot back to the member and the pool.
    let view = pilot.limits(20.0);
    assert_eq!(
        (view.allowance.members[0].used, view.allowance.pool.used),
        (1, 1)
    );
    assert!(view.allowance.members[0].overridden);
    pilot.allowance.refund("11", 10.0);
    let view = pilot.limits(20.0);
    assert_eq!(
        (view.allowance.members[0].used, view.allowance.pool.used),
        (0, 0)
    );
    assert!(
        decide(
            &asked_by("11"),
            &settings,
            &channels,
            summons(),
            pilot.allowance.budgets(21.0)
        )
        .act
    );
}

#[test]
fn one_answer_per_channel_with_a_bounded_fair_queue() {
    let mut pilot = ChatPilot::new(
        2700.0,
        TrafficLimits {
            per_channel: 2,
            guild: 3,
            max_wait_s: 60.0,
        },
        GuardLimits::default(),
    );
    let traffic = &mut pilot.traffic;
    assert_eq!(
        traffic.admit("700", "m1", "11", (0.0, 0.0)),
        Admission::Answer
    );
    assert_eq!(
        traffic.admit("700", "m2", "22", (1.0, 1.0)),
        Admission::Queued { position: 1 }
    );
    assert_eq!(
        traffic.admit("700", "m3", "33", (2.0, 2.0)),
        Admission::Queued { position: 2 }
    );
    assert_eq!(
        traffic.admit("700", "m4", "44", (3.0, 3.0)),
        Admission::Busy,
        "channel bound"
    );
    assert_eq!(
        traffic.admit("702", "n1", "11", (3.0, 3.0)),
        Admission::Answer
    );
    assert_eq!(
        traffic.admit("702", "n2", "22", (4.0, 4.0)),
        Admission::Queued { position: 1 }
    );
    assert_eq!(
        traffic.admit("702", "n3", "33", (5.0, 5.0)),
        Admission::Busy,
        "guild bound"
    );

    // Deleting a waiting question removes it; the queue closes up.
    assert_eq!(traffic.cancel("m2").map(|w| w.member_id), Some("22".into()));
    assert_eq!(traffic.position("m3"), Some(1));
    assert_eq!(
        traffic.finish("700").map(|w| w.message_id),
        Some("m3".into())
    );
    assert_eq!(traffic.finish("700"), None, "the channel is free again");
    assert_eq!(
        traffic.admit("700", "m5", "44", (10.0, 10.0)),
        Admission::Answer
    );

    // Waiting past the bound gives up (busy reaction and refund upstream).
    let dropped = traffic.expire(65.0);
    assert_eq!(
        dropped
            .iter()
            .map(|w| w.message_id.as_str())
            .collect::<Vec<_>>(),
        ["n2"]
    );
    let view = pilot.limits(65.0);
    assert_eq!(view.queue.answering, ["700", "702"]);
    assert!(view.queue.waiting.is_empty());
}

#[test]
fn clean_retries_are_limited_per_member_and_by_a_storm_guard() {
    let mut pilot = new_pilot();
    assert!(pilot.clean_retry_allowed("11", 0.0));
    assert_eq!(pilot.guard.record("11", 0.0), None);
    assert!(
        !pilot.clean_retry_allowed("11", 59.0),
        "once per member per 10 min"
    );
    for (member, at) in [("22", 1.0), ("33", 2.0)] {
        assert_eq!(pilot.guard.record(member, at), None);
    }
    let alert = pilot
        .guard
        .record("44", 3.0)
        .expect("the fourth within a minute trips");
    assert_eq!((alert.retries, alert.suspended_until), (4, 603.0));
    assert!(
        !pilot.clean_retry_allowed("55", 10.0),
        "suspended guild-wide"
    );
    assert_eq!(pilot.guard.record("66", 11.0), None, "one alert per trip");
    assert!(pilot.clean_retry_allowed("55", 603.0));
    assert!(pilot.clean_retry_allowed("11", 603.0), "ten minutes on");
    assert_eq!(pilot.limits(603.0).clean_retry.suspended_until, None);
}

#[test]
fn routing_is_code_only() {
    let offer = ChatPilot::route("can we move hstar to friday?", None, false);
    assert_eq!(offer.bundles(), [Bundle::Read, Bundle::RunWrites]);
    let offer = ChatPilot::route("ok", Some(CardContext::Weekly), true);
    assert!(
        !offer.offers(ToolName::ProposeChangeFixed),
        "never writes on a read-only turn"
    );
}

fn message(
    id: &str,
    author: &str,
    content: &str,
    reply_to: Option<(&str, &str, &str)>,
) -> QuestionMessage {
    QuestionMessage {
        id: id.into(),
        author_id: author.into(),
        content: content.into(),
        reference: reply_to.map(|(parent, parent_author, text)| Reference {
            message_id: Some(parent.into()),
            resolved: Some(Box::new(kanade::chat::context::Parent {
                id: parent.into(),
                author_id: Some(parent_author.into()),
                content: Some(text.into()),
                reference: None,
            })),
        }),
    }
}

fn answered(reply: &str) -> Generation {
    Generation {
        reply: reply.into(),
        ..Generation::default()
    }
}

async fn world() -> World {
    World::new(&load("read_tools.json")["cases"][0]["input"]).await
}

#[tokio::test]
async fn an_answer_is_remembered_anchored_and_focused() {
    let world = world().await;
    let persona = kanade();
    let mut pilot = new_pilot();
    let replies = Replies::default();
    let question = message("8100", "11", "what's on?", None);
    let mut generation = answered("Nothing much!");
    generation.focus = Some("move Hard Baldrix to Sat 12 Sep 21:00 — Alvin tan".into());
    let concluded = pilot
        .conclude(
            Finished {
                message: &question,
                channel_id: "700",
                ctx: &ctx("11"),
                generation: &generation,
                persona: &persona,
                directory: &world.guild,
                log: facts("chat-1"),
                spent_at: Some(1.0),
                now: 5.0,
            },
            &replies,
        )
        .await;
    assert_eq!(concluded.posted_id.as_deref(), Some("reply-1"));
    assert_eq!(
        replies.0.lock().unwrap()[0],
        ("700".into(), "8100".into(), "Nothing much!".into())
    );
    let history = pilot.conversations.history("700", 6.0);
    assert_eq!(
        history
            .iter()
            .map(|t| (t.content.as_str(), t.message_id.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("Alvin tan: what's on?", Some("8100")),
            ("Nothing much!", Some("reply-1"))
        ]
    );
    assert_eq!(
        pilot.conversations.focus("700", 6.0),
        generation.focus.clone().unwrap()
    );
    assert_eq!(concluded.interaction.outcome, ChatOutcome::Answered);
    assert!(!concluded.withheld);
    // After the history ages out, replying to the (uncached) answer re-anchors it.
    let mut later = message("8101", "22", "and then?", None);
    later.reference = Some(Reference {
        message_id: Some("reply-1".into()),
        resolved: None,
    });
    let turns = build_turns(
        &mut pilot.conversations,
        &later,
        "700",
        5.0 + 2700.0,
        BOT,
        &world.guild,
    );
    assert_eq!(
        turns.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(),
        ["Alvin tan: what's on?", "Nothing much!", "kanon: and then?"]
    );
}

/// A explicit → B unaffected: A's blocked question and the reply to it are
/// posted as the fixed line but never reach anyone's context, not through
/// history, an anchor or a reply chain.
#[tokio::test]
async fn blocked_content_is_withheld_from_every_later_context() {
    let world = world().await;
    let persona = kanade();
    let mut pilot = new_pilot();
    let replies = Replies::default();
    let explicit = message("8200", "22", "something explicit", None);
    let blocked = Generation {
        failure: Some(AnswerFailure::ContentBlocked),
        clean_retry: true,
        ..Generation::default()
    };
    let concluded = pilot
        .conclude(
            Finished {
                message: &explicit,
                channel_id: "700",
                ctx: &ctx("22"),
                generation: &blocked,
                persona: &persona,
                directory: &world.guild,
                log: facts("chat-a"),
                spent_at: Some(1.0),
                now: 5.0,
            },
            &replies,
        )
        .await;
    assert_eq!(
        concluded.reply, CONTENT_BLOCKED_REPLY,
        "the tracked bundle has no line of its own"
    );
    assert!(concluded.withheld);
    assert_eq!(
        (
            concluded.interaction.outcome,
            concluded.interaction.withheld
        ),
        (ChatOutcome::ContentBlocked, true)
    );
    assert_eq!(
        concluded.interaction.guardrail,
        json!({"content_filter": true})
    );

    let b = message(
        "8201",
        "33",
        "what's on?",
        Some(("8200", "22", "something explicit")),
    );
    let turns = build_turns(&mut pilot.conversations, &b, "700", 6.0, BOT, &world.guild);
    let texts: Vec<&str> = turns.iter().map(|t| t.prompt_text()).collect();
    assert_eq!(texts, [WITHHELD, WITHHELD, "Priya: what's on?"]);
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("explicit") || t.contains(CONTENT_BLOCKED_REPLY))
    );
    // Replying to the bot's line never re-anchors the exchange.
    let reply_to_bot = message(
        "8202",
        "33",
        "why?",
        Some(("reply-1", BOT, CONTENT_BLOCKED_REPLY)),
    );
    let turns = build_turns(
        &mut pilot.conversations,
        &reply_to_bot,
        "700",
        6.0 + 2700.0,
        BOT,
        &world.guild,
    );
    assert_eq!(
        turns.iter().map(|t| t.prompt_text()).collect::<Vec<_>>(),
        [WITHHELD, "Priya: why?"]
    );
}

#[tokio::test]
async fn other_failures_say_v4s_line_and_turned_away_questions_are_refunded() {
    let world = world().await;
    let persona = kanade();
    let mut pilot = new_pilot();
    let (channels, settings) = gate_input();
    let decision = decide(
        &asked_by("11"),
        &settings,
        &channels,
        summons(),
        pilot.allowance.budgets(1.0),
    );
    assert!(decision.act);
    let turned_away = Generation::failed(AnswerFailure::Session(SessionError {
        failure: SessionFailure::Refused(Refused::Timeout),
        charge: Charge::Refunded,
    }));
    let question = message("8300", "11", "what's on?", None);
    let concluded = pilot
        .conclude(
            Finished {
                message: &question,
                channel_id: "700",
                ctx: &ctx("11"),
                generation: &turned_away,
                persona: &persona,
                directory: &world.guild,
                log: facts("chat-t"),
                spent_at: Some(1.0),
                now: 2.0,
            },
            &Replies::default(),
        )
        .await;
    assert_eq!(concluded.reply, FAILURE_REPLY);
    assert_eq!(concluded.interaction.outcome, ChatOutcome::TurnedAway);
    assert_eq!(pilot.limits(2.0).allowance.pool.used, 0, "refunded");
    assert!(!concluded.withheld);
}

/// With the guard off, a content-filtered answer is not retried: one
/// request, the fixed line, withheld.
#[tokio::test(start_paused = true)]
async fn a_guarded_off_clean_retry_is_never_sent() {
    let input = load("loop.json")["cases"][0]["input"].clone();
    let mut world = World::new(&input).await;
    let filtered = || {
        FakeAction::Response(CompletionResponse {
            model: MODEL.into(),
            content: None,
            tool_calls: Vec::new(),
            finish_reason: FinishReason::ContentFilter,
            usage: None,
        })
    };
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new([filtered(), filtered()]),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let mut pilot = new_pilot();
    pilot.guard.record("11", 0.0);
    let ask = ctx("11");
    let mut settings = settings(&input, 8);
    settings.clean_retry = pilot.clean_retry_allowed("11", 1.0);
    assert!(!settings.clean_retry);
    let persona = kanade();
    let roster = roster(&world);
    let deps = AnswerDeps {
        client: &client,
        codec: &Passthrough,
        roster: &roster,
    };
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ask,
            conversation: vec![
                Message::System {
                    content: "SYSTEM".into(),
                },
                Message::User {
                    content: "Alvin tan: hmm".into(),
                },
            ],
            reminder: persona.voice_reminder(),
            offer: ToolOffer::full_set(false),
            settings,
        };
        answer(&deps, question, &guild, &mut proposer, &Ports::default()).await
    };
    assert_eq!(provider.fake.requests().len(), 1);
    assert_eq!(generation.failure, Some(AnswerFailure::ContentBlocked));
    assert!(!generation.clean_retry);
    let question = message("8400", "11", "hmm", None);
    let concluded = pilot
        .conclude(
            Finished {
                message: &question,
                channel_id: "700",
                ctx: &ask,
                generation: &generation,
                persona: &persona,
                directory: &world.guild,
                log: facts("chat-g"),
                spent_at: None,
                now: 1.0,
            },
            &Replies::default(),
        )
        .await;
    assert_eq!(concluded.reply, CONTENT_BLOCKED_REPLY);
    assert!(concluded.withheld && concluded.alert.is_none());
}
