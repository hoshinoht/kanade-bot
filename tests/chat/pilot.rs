//! C3 traffic and safety around a question: allowance overrides, refunds
//! and the once-per-episode limited reply; the per-channel queue; the
//! clean-retry storm guard; code-only routing; the post-answer glue with
//! pollution containment; failure lines; and the Limits view.

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use kanade::chat::answer::{
    AnswerDeps, AnswerFailure, Generation, ProfanityGuard, ProfanityHit, ProfanitySide, Question,
    answer,
};
use kanade::chat::context::{ChatTurn, QuestionMessage, Reference, WITHHELD, build_turns};
use kanade::chat::gate::{
    Author, ChannelInfo, ChatDecision, IncomingMessage, RATE_LIMITED, Summons, decide,
};
use kanade::chat::pilot::{
    Admission, CONTENT_BLOCKED_REPLY, ChatPilot, Concluded, Finished, GuardLimits, LogFacts,
    ReplyPort, TrafficLimits,
};
use kanade::chat::sanitize::FAILURE_REPLY;
use kanade::chat::tools::bundles::{Bundle, CardContext, ToolOffer};
use kanade::chat::tools::{ToolContext, ToolName};
use kanade::infrastructure::llm::governor::{Charge, Refused, SessionError, SessionFailure};
use kanade::infrastructure::llm::{FakeAction, FakeProvider};
use kanade::infrastructure::store::SqliteStore;
use kanade_domain::model_log::ModelLogStore;
use kanade_domain::model_log::{AllowanceOverride, ChatOutcome};
use serde_json::json;

use crate::looping::{Ports, conversation, settings};
use crate::model::{Scripted, capabilities, client};
use crate::support::world_input;
use crate::wire::kanade;
use crate::world::{Channels, World, pilot};

const BOT: &str = "5000";
const ROLE: &str = "5001";
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

/// The gate's decision on member 11's summons in channel 700 at `now`.
fn decide_at(chat: &mut ChatPilot, now: f64) -> ChatDecision {
    let input = world_input();
    let (channels, settings) = (Channels::new(&input["channels"]), pilot(&input["settings"]));
    let asked = IncomingMessage {
        author: Some(Author {
            id: "11".into(),
            bot: false,
            roles: vec!["6000".into()],
        }),
        guild_id: Some("1000".into()),
        channel: Some(ChannelInfo::bare("700")),
        mentions: vec![BOT.into()],
        role_mentions: Vec::new(),
    };
    let summons = Summons {
        bot_user_id: Some(BOT),
        self_role_id: None,
        replied_author_id: None,
        enabled: true,
        is_admin: false,
    };
    decide(
        &asked,
        &settings,
        &channels,
        summons,
        chat.allowance.budgets(now),
    )
}

fn ctx_for(member: &str, message_id: &str) -> ToolContext {
    let mut ctx = ToolContext::new(
        member,
        "700",
        message_id,
        Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap(),
    );
    ctx.bot_user_id = Some(BOT.into());
    ctx.self_role_id = Some(ROLE.into());
    ctx
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
    assert!(decide_at(&mut pilot, 10.0).act);
    let second = decide_at(&mut pilot, 20.0);
    assert_eq!((second.reason, second.retry_after_s), (RATE_LIMITED, 50.0));

    let ctx = ctx_for("11", "8100");
    let (reply, row) = pilot.limited(&ctx, "again?", &second, facts("limited-1"), 20.0);
    assert_eq!(
        reply.as_deref(),
        Some("That's your 1 answer for now — ask me again in about 50s."),
        "their own override, not the default"
    );
    assert_eq!(row.outcome, ChatOutcome::RateLimited);
    let (again, row) = pilot.limited(&ctx, "again?", &second, facts("limited-2"), 25.0);
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
    assert!(decide_at(&mut pilot, 21.0).act);
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
    let at = |now: f64| (now, Some(now));
    let queued = |position| Admission::Queued { position };
    assert_eq!(traffic.admit("700", "m1", "11", at(0.0)), Admission::Answer);
    assert_eq!(traffic.admit("700", "m2", "22", at(1.0)), queued(1));
    assert_eq!(
        traffic.admit("700", "m3", "33", (2.0, None)),
        queued(2),
        "an admin spends no allowance"
    );
    assert_eq!(
        traffic.admit("700", "m4", "44", at(3.0)),
        Admission::Busy,
        "channel bound"
    );
    assert_eq!(traffic.admit("702", "n1", "11", at(3.0)), Admission::Answer);
    assert_eq!(traffic.admit("702", "n2", "22", at(4.0)), queued(1));
    assert_eq!(
        traffic.admit("702", "n3", "33", at(5.0)),
        Admission::Busy,
        "guild bound"
    );

    // Deleting a waiting question removes it; the queue closes up.
    assert_eq!(traffic.cancel("m2").map(|w| w.member_id), Some("22".into()));
    assert_eq!(traffic.position("m3"), Some(1));
    let handoff = traffic.finish("700", 6.0);
    assert_eq!(
        (handoff.next.map(|w| w.message_id), handoff.expired),
        (Some("m3".into()), Vec::new())
    );
    assert_eq!(
        traffic.finish("700", 7.0).next,
        None,
        "the channel is free again"
    );
    assert_eq!(
        traffic.admit("700", "m5", "44", at(10.0)),
        Admission::Answer
    );
    assert_eq!(traffic.admit("700", "m6", "55", at(11.0)), queued(1));

    // Finishing gives up stale waiters first, so a question that outwaited
    // its bound is never handed over.
    let handoff = traffic.finish("700", 71.0);
    assert_eq!(handoff.next, None);
    assert_eq!(
        handoff
            .expired
            .iter()
            .map(|w| (w.message_id.as_str(), w.spent_at))
            .collect::<Vec<_>>(),
        [("m6", Some(11.0)), ("n2", Some(4.0))],
        "by channel"
    );
    assert_eq!(
        traffic.admit("700", "m5", "44", at(72.0)),
        Admission::Answer
    );
    assert_eq!(traffic.admit("702", "n4", "22", at(72.0)), queued(1));

    // Waiting past the bound gives up (busy reaction and refund upstream).
    let dropped = traffic.expire(140.0);
    assert_eq!(
        dropped
            .iter()
            .map(|w| w.message_id.as_str())
            .collect::<Vec<_>>(),
        ["n4"]
    );
    let view = pilot.limits(140.0);
    assert_eq!(view.queue.answering, ["700", "702"]);
    assert!(view.queue.waiting.is_empty());
}

#[test]
fn clean_retries_are_limited_per_member_and_by_a_storm_guard() {
    let mut pilot = new_pilot();
    assert!(pilot.reserve_clean_retry("11", 0.0));
    assert_eq!(pilot.guard.settle("11", true, 0.0), None);
    assert!(
        !pilot.reserve_clean_retry("11", 59.0),
        "once per member per 10 min"
    );
    for (member, at) in [("22", 1.0), ("33", 2.0)] {
        assert!(pilot.reserve_clean_retry(member, at));
        assert_eq!(pilot.guard.settle(member, true, at), None);
    }
    assert!(pilot.reserve_clean_retry("44", 3.0));
    let alert = pilot
        .guard
        .settle("44", true, 3.0)
        .expect("the fourth within a minute trips");
    assert_eq!((alert.retries, alert.suspended_until), (4, 603.0));
    assert!(
        !pilot.reserve_clean_retry("55", 10.0),
        "suspended guild-wide"
    );
    assert_eq!(pilot.guard.record("66", 11.0), None, "one alert per trip");
    assert!(pilot.reserve_clean_retry("55", 603.0));
    assert!(pilot.reserve_clean_retry("11", 603.0), "ten minutes on");
    let view = pilot.limits(603.0).clean_retry;
    assert_eq!((view.suspended_until, view.pending), (None, 2));
}

#[test]
fn concurrent_questions_from_one_member_reserve_one_clean_retry() {
    let mut pilot = new_pilot();
    assert!(pilot.reserve_clean_retry("11", 0.0));
    assert!(
        !pilot.reserve_clean_retry("11", 0.5),
        "the second question in flight gets none"
    );
    // The first (the holder) never sent its retry: the reservation is released.
    assert_eq!(pilot.guard.settle("11", false, 1.0), None);
    assert!(pilot.reserve_clean_retry("11", 2.0));
    assert_eq!(pilot.guard.settle("11", true, 3.0), None);
    assert!(!pilot.reserve_clean_retry("11", 4.0), "spent, not released");
}

/// A (reserved) and B (refused) from one member in two channels; B ends
/// first. Only A's conclusion may settle the reservation.
#[tokio::test]
async fn a_refused_question_never_releases_anothers_reservation() {
    let world = world().await;
    let mut pilot = new_pilot();
    let a_reserved = pilot.reserve_clean_retry("11", 0.0);
    let b_reserved = pilot.reserve_clean_retry("11", 0.5);
    assert!(a_reserved && !b_reserved);
    let b = message("b", "11", "hmm", None);
    let spent = Spent {
        at: None,
        reserved: b_reserved,
    };
    conclude_paid(
        &mut pilot,
        &world,
        &b,
        &answered("B."),
        1.0,
        spent,
        &ReplyTo,
    )
    .await;
    assert_eq!(pilot.limits(1.0).clean_retry.pending, 1, "A still holds it");
    assert!(!pilot.reserve_clean_retry("11", 2.0), "C is still refused");
    let sent = Generation {
        clean_retry: true,
        ..answered("A.")
    };
    let a = message("a", "11", "hmm", None);
    let spent = Spent {
        at: None,
        reserved: a_reserved,
    };
    conclude_paid(&mut pilot, &world, &a, &sent, 3.0, spent, &ReplyTo).await;
    let view = pilot.limits(3.0).clean_retry;
    assert_eq!((view.pending, view.recent), (0, 1));
    assert!(!pilot.reserve_clean_retry("11", 602.0), "inside 600 s");
    assert!(pilot.reserve_clean_retry("11", 603.0));
}

#[test]
fn questions_in_many_channels_cannot_overrun_the_storm_limit() {
    let mut pilot = new_pilot();
    let members = ["11", "22", "33", "44", "55", "66"];
    let reserved: Vec<bool> = members
        .iter()
        .map(|member| pilot.reserve_clean_retry(member, 0.0))
        .collect();
    assert_eq!(
        reserved,
        [true, true, true, true, false, false],
        "at most the tripping fourth is reserved beside the other three"
    );
    let alerts: Vec<_> = members[..4]
        .iter()
        .filter_map(|member| pilot.guard.settle(member, true, 1.0))
        .collect();
    assert_eq!(alerts.len(), 1, "one alert");
    assert_eq!(alerts[0].retries, 4);
    assert!(!pilot.reserve_clean_retry("55", 2.0), "suspended");
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
    World::new(&world_input()).await
}

/// The turns `message` in channel 700 is answered with at `now`.
fn turns(
    chat: &mut ChatPilot,
    world: &World,
    message: &QuestionMessage,
    now: f64,
) -> Vec<ChatTurn> {
    build_turns(
        &mut chat.conversations,
        message,
        "700",
        now,
        BOT,
        None,
        &world.guild,
    )
}

fn texts(turns: &[ChatTurn]) -> Vec<String> {
    turns.iter().map(|t| t.prompt_text().to_owned()).collect()
}

/// What a concluded question spent: when its allowance slot was taken
/// (refunded on a turned-away answer) and whether it held the clean retry.
#[derive(Clone, Copy, Default)]
struct Spent {
    at: Option<f64>,
    reserved: bool,
}

/// An allowance slot taken at 1 s, no clean retry held.
const SPENT: Spent = Spent {
    at: Some(1.0),
    reserved: false,
};

/// Concludes `question` on `chat` in channel 700 at `now`, as its author.
async fn conclude_paid(
    chat: &mut ChatPilot,
    world: &World,
    question: &QuestionMessage,
    generation: &Generation,
    now: f64,
    spent: Spent,
    replies: &impl ReplyPort,
) -> Concluded {
    chat.conclude(
        Finished {
            message: question,
            channel_id: "700",
            ctx: &ctx_for(&question.author_id, &question.id),
            generation,
            persona: &kanade(),
            directory: &world.guild,
            log: facts(&format!("chat-{}", question.id)),
            spent_at: spent.at,
            reserved: spent.reserved,
            now,
        },
        replies,
    )
    .await
}

/// [`conclude_paid`] spending nothing, replying through [`ReplyTo`].
async fn conclude_on(
    chat: &mut ChatPilot,
    world: &World,
    question: &QuestionMessage,
    generation: &Generation,
    now: f64,
) -> Concluded {
    conclude_paid(
        chat,
        world,
        question,
        generation,
        now,
        Spent::default(),
        &ReplyTo,
    )
    .await
}

#[tokio::test]
async fn an_answer_is_remembered_anchored_and_focused() {
    let world = world().await;
    let mut pilot = new_pilot();
    let replies = Replies::default();
    let question = message("8100", "11", "what's on?", None);
    let mut generation = answered("Nothing much!");
    generation.focus = Some("move Hard Baldrix to Sat 12 Sep 21:00 — Alvin tan".into());
    let concluded = conclude_paid(
        &mut pilot,
        &world,
        &question,
        &generation,
        5.0,
        SPENT,
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
    let turns = turns(&mut pilot, &world, &later, 5.0 + 2700.0);
    assert_eq!(
        turns.iter().map(|t| t.content.as_str()).collect::<Vec<_>>(),
        ["Alvin tan: what's on?", "Nothing much!", "kanon: and then?"]
    );
}

#[tokio::test]
async fn remembered_questions_strip_leading_own_mentions_only() {
    let world = world().await;
    let mut pilot = new_pilot();
    let question = message("8110", "11", "<@&5001>, what are my runs <@22>?", None);
    let concluded = conclude_on(&mut pilot, &world, &question, &answered("Checking."), 5.0).await;
    assert_eq!(
        pilot.conversations.history("700", 6.0)[0].content,
        "Alvin tan: what are my runs <@22>?"
    );
    assert_eq!(
        concluded.interaction.question, "<@&5001>, what are my runs <@22>?",
        "the chat log retains the raw source"
    );
}

/// A explicit → B unaffected: A's blocked question and the reply to it are
/// posted as the fixed line but never reach anyone's context, not through
/// history, an anchor or a reply chain.
#[tokio::test]
async fn blocked_content_is_withheld_from_every_later_context() {
    let world = world().await;
    let mut pilot = new_pilot();
    let explicit = message("8200", "22", "something explicit", None);
    let blocked = Generation {
        failure: Some(AnswerFailure::ContentBlocked),
        clean_retry: true,
        ..Generation::default()
    };
    let replies = Replies::default();
    let concluded = conclude_paid(
        &mut pilot, &world, &explicit, &blocked, 5.0, SPENT, &replies,
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
    let texts_b = texts(&turns(&mut pilot, &world, &b, 6.0));
    assert_eq!(texts_b, [WITHHELD, WITHHELD, "Priya: what's on?"]);
    assert!(
        !texts_b
            .iter()
            .any(|t| t.contains("explicit") || t.contains(CONTENT_BLOCKED_REPLY))
    );
    // Replying to the bot's line never re-anchors the exchange.
    let parent = Some(("reply-1", BOT, CONTENT_BLOCKED_REPLY));
    let reply_to_bot = message("8202", "33", "why?", parent);
    assert_eq!(
        texts(&turns(&mut pilot, &world, &reply_to_bot, 6.0 + 2700.0)),
        [WITHHELD, "Priya: why?"]
    );
}

#[tokio::test]
async fn other_failures_say_v4s_line_and_turned_away_questions_are_refunded() {
    let world = world().await;
    let mut pilot = new_pilot();
    assert!(decide_at(&mut pilot, 1.0).act);
    let turned_away = Generation::failed(AnswerFailure::Session(SessionError {
        failure: SessionFailure::Refused(Refused::Timeout),
        charge: Charge::Refunded,
    }));
    let question = message("8300", "11", "what's on?", None);
    let replies = Replies::default();
    let concluded = conclude_paid(
        &mut pilot,
        &world,
        &question,
        &turned_away,
        2.0,
        SPENT,
        &replies,
    )
    .await;
    assert_eq!(concluded.reply, FAILURE_REPLY);
    assert_eq!(concluded.interaction.outcome, ChatOutcome::TurnedAway);
    assert_eq!(pilot.limits(2.0).allowance.pool.used, 0, "refunded");
    assert!(!concluded.withheld);
}

fn filtered() -> FakeAction {
    crate::model::filtered(MODEL)
}

/// A filtered first round, then `after` for the clean retry, concluded:
/// the generation, the requests sent and what `conclude` did.
async fn filtered_then(
    chat: &mut ChatPilot,
    after: Vec<FakeAction>,
    message_id: &str,
) -> (Generation, usize, Concluded) {
    let input = world_input();
    let mut world = World::new(&input).await;
    let provider = Arc::new(Scripted {
        fake: FakeProvider::new(std::iter::once(filtered()).chain(after)),
        caps: capabilities(&input["caps"]),
    });
    let (_governor, client) = client(Some(MODEL), provider.clone());
    let ask = ctx_for("11", message_id);
    let mut settings = settings(&input, 8);
    let reserved = chat.reserve_clean_retry("11", 1.0);
    settings.clean_retry = reserved;
    let deps = AnswerDeps {
        client: &client,
        route: None,
    };
    let generation = {
        let (guild, mut proposer) = world.question_parts();
        let question = Question {
            ctx: &ask,
            conversation: conversation("Alvin tan: hmm".into()),
            profanity: None,
            reminder: kanade().voice_reminder(),
            offer: ToolOffer::full_set(false),
            settings,
        };
        answer(&deps, question, &guild, &mut proposer, &Ports::default()).await
    };
    let question = message(message_id, "11", "hmm", None);
    let spent = Spent { at: None, reserved };
    let concluded = conclude_paid(chat, &world, &question, &generation, 2.0, spent, &ReplyTo).await;
    (generation, provider.fake.requests().len(), concluded)
}

fn assert_withheld(concluded: &Concluded) {
    assert_eq!(concluded.reply, CONTENT_BLOCKED_REPLY);
    assert!(concluded.withheld);
    assert!(concluded.interaction.withheld);
    assert_eq!(
        concluded.interaction.guardrail,
        json!({"content_filter": true})
    );
}

/// With the guard off, a content-filtered answer is not retried: one
/// request, the fixed line, withheld.
#[tokio::test(start_paused = true)]
async fn a_guarded_off_clean_retry_is_never_sent() {
    let mut pilot = new_pilot();
    pilot.guard.record("11", 0.0);
    let (generation, requests, concluded) =
        filtered_then(&mut pilot, vec![filtered()], "8400").await;
    assert_eq!(requests, 1);
    assert_eq!(generation.failure, Some(AnswerFailure::ContentBlocked));
    assert!(!generation.clean_retry);
    assert_withheld(&concluded);
    assert!(concluded.alert.is_none());
}

/// Filtered, then the clean retry is sent and times out: still blocked
/// content (sticky), so withheld with the blocked line, not v4's line.
#[tokio::test(start_paused = true)]
async fn a_filtered_round_whose_clean_retry_times_out_stays_withheld() {
    let mut pilot = new_pilot();
    let (generation, requests, concluded) =
        filtered_then(&mut pilot, vec![FakeAction::UpstreamTimeout], "8401").await;
    assert_eq!(requests, 2);
    assert!(generation.clean_retry && generation.blocked);
    assert!(matches!(
        generation.failure,
        Some(AnswerFailure::Timeout { .. })
    ));
    assert_withheld(&concluded);
    assert_eq!(concluded.interaction.outcome, ChatOutcome::Timeout);
    assert!(pilot.conversations.is_withheld("8401"));
    assert!(
        !pilot.reserve_clean_retry("11", 3.0),
        "the sent retry was counted"
    );
}

#[tokio::test(start_paused = true)]
async fn a_filtered_round_whose_clean_retry_is_malformed_stays_withheld() {
    let mut pilot = new_pilot();
    let malformed = vec![FakeAction::Malformed; 4];
    let (generation, requests, concluded) = filtered_then(&mut pilot, malformed, "8402").await;
    assert!(requests >= 2);
    assert!(generation.clean_retry && generation.blocked);
    assert_eq!(generation.failure, Some(AnswerFailure::Malformed));
    assert_withheld(&concluded);
    assert!(pilot.conversations.is_withheld("8402"));
}

/// After a restart the pilot's memory is empty; reloading the chat log's
/// withheld rows keeps a direct reply from pulling the text back.
#[tokio::test]
async fn withheld_questions_survive_a_restart() {
    let world = world().await;
    let store = SqliteStore::open_temp().await;
    let mut before = new_pilot();
    let explicit = message("8500", "22", "something explicit", None);
    let blocked = Generation::failed(AnswerFailure::ContentBlocked);
    let concluded = conclude_on(&mut before, &world, &explicit, &blocked, 1.0).await;
    store.record_chat(concluded.interaction).await.unwrap();
    let fine = message("8501", "22", "fine question", None);
    let mut answered_row = conclude_on(&mut before, &world, &fine, &answered("Sure."), 2.0)
        .await
        .interaction;
    answered_row.at += chrono::TimeDelta::seconds(1);
    store.record_chat(answered_row).await.unwrap();

    let parent = Some(("8500", "22", "something explicit"));
    let reply = message("8502", "33", "what did they say?", parent);
    let mut restarted = new_pilot();
    assert_eq!(
        turns(&mut restarted, &world, &reply, 3.0)[0].prompt_text(),
        "kanon: something explicit",
        "without the reload the parent comes back"
    );
    let mut restarted = new_pilot();
    assert_eq!(restarted.reload_withheld(&store).await.unwrap(), 1);
    assert!(!restarted.conversations.is_withheld("8501"));
    assert_eq!(
        texts(&turns(&mut restarted, &world, &reply, 3.0)),
        [WITHHELD, "Priya: what did they say?"]
    );
}

/// Posts the answer as `reply-to-<question id>`: unique per question.
struct ReplyTo;

impl ReplyPort for ReplyTo {
    async fn post_reply(
        &self,
        _channel_id: &str,
        reply_to: &str,
        _text: &str,
    ) -> Result<String, String> {
        Ok(format!("reply-to-{reply_to}"))
    }
}

const LINE: &str = "Language, please!";

fn line_guard() -> ProfanityGuard {
    ProfanityGuard::new(&kanade_domain::settings::Profanity {
        deflection_line: LINE.into(),
        ..kanade_domain::settings::Profanity::default()
    })
}

/// A reply whose profanity check caught `shit`; `sent` is what replaced it.
fn caught_reply(reply: &str, sent: Option<&str>) -> Generation {
    Generation {
        reply: reply.into(),
        profanity: Some(ProfanityHit {
            side: ProfanitySide::Reply,
            word: "shit".into(),
            sent: sent.map(Into::into),
        }),
        ..Generation::default()
    }
}

/// A deflected question and the line sent for it never enter later context:
/// not history, not anchors, not a reply chain to either message, and no
/// placeholder either. The log row keeps the question in full.
#[tokio::test]
async fn a_deflected_question_and_its_line_stay_out_of_context() {
    let world = world().await;
    let mut pilot = new_pilot();
    let rude = message("8600", "22", "this fucking bot, when is lotus?", None);
    let deflected = line_guard().deflect("fuck".into());
    let concluded = conclude_on(&mut pilot, &world, &rude, &deflected, 1.0).await;
    assert_eq!(concluded.reply, LINE);
    assert!(!concluded.withheld, "not the content-filter mechanism");
    assert_eq!(concluded.interaction.outcome, ChatOutcome::Profanity);
    assert_eq!(
        concluded.interaction.question, rude.content,
        "logged in full"
    );
    assert!(!concluded.interaction.withheld);
    let posted = concluded.posted_id.expect("posted");

    let next = message("8601", "33", "what's on?", None);
    assert_eq!(
        texts(&turns(&mut pilot, &world, &next, 2.0)),
        ["Priya: what's on?"]
    );
    for (parent, author, text) in [
        ("8600", "22", rude.content.as_str()),
        (posted.as_str(), BOT, LINE),
    ] {
        let reply = message("8602", "33", "huh?", Some((parent, author, text)));
        let turns = turns(&mut pilot, &world, &reply, 2.0);
        assert_eq!(texts(&turns), ["Priya: huh?"], "reply to {parent}");
    }
}

/// A reply replaced by the safe line keeps the whole exchange out (the
/// question alone would dangle unanswered); a reply whose clean retry was
/// delivered is a normal exchange and stays.
#[tokio::test]
async fn a_replaced_reply_drops_its_exchange_and_a_recovered_one_keeps_it() {
    let world = world().await;
    let mut pilot = new_pilot();
    let replaced = Generation {
        clean_retry: true,
        ..caught_reply(LINE, Some(LINE))
    };
    let asked = message("8700", "22", "when is kalos?", None);
    conclude_on(&mut pilot, &world, &asked, &replaced, 1.0).await;
    let recovered = Generation {
        clean_retry: true,
        ..caught_reply("Lotus is at nine.", None)
    };
    let asked = message("8701", "22", "when is lotus?", None);
    conclude_on(&mut pilot, &world, &asked, &recovered, 2.0).await;
    let next = message("8702", "33", "and after?", None);
    let texts = texts(&turns(&mut pilot, &world, &next, 3.0));
    assert_eq!(
        texts,
        [
            "kanon: when is lotus?",
            "Lotus is at nine.",
            "Priya: and after?"
        ]
    );
    assert!(
        !texts
            .iter()
            .any(|t| t.contains("kalos") || t.contains(LINE))
    );
}

/// Like withheld ids, excluded question ids come back from the chat log after
/// a restart, so a direct reply cannot pull a deflected question back in.
#[tokio::test]
async fn excluded_questions_survive_a_restart() {
    let world = world().await;
    let store = SqliteStore::open_temp().await;
    let mut before = new_pilot();
    let rude = message("8800", "22", "this fucking bot", None);
    let deflected = line_guard().deflect("fuck".into());
    let concluded = conclude_on(&mut before, &world, &rude, &deflected, 1.0).await;
    store.record_chat(concluded.interaction).await.unwrap();
    let asked = message("8801", "22", "when is lotus?", None);
    let mut recovered = conclude_on(
        &mut before,
        &world,
        &asked,
        &caught_reply("Nine.", None),
        2.0,
    )
    .await
    .interaction;
    recovered.at += chrono::TimeDelta::seconds(1);
    store.record_chat(recovered).await.unwrap();

    let reply = message(
        "8802",
        "33",
        "what?",
        Some(("8800", "22", "this fucking bot")),
    );
    let mut restarted = new_pilot();
    assert_eq!(restarted.reload_excluded(&store).await.unwrap(), 1);
    assert!(
        !restarted.conversations.is_excluded("8801"),
        "recovered stays"
    );
    assert_eq!(
        texts(&turns(&mut restarted, &world, &reply, 3.0)),
        ["Priya: what?"]
    );
}
