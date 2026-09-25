//! The driver over a scripted answerer and a recording surface; the
//! monotonic clock is the test's.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{NaiveTime, TimeZone, Utc, Weekday};
use serde_json::json;
use tokio::sync::Notify;

use super::*;
use crate::chat::answer::Generation;
use crate::chat::context::{Parent, QuestionMessage, Reference, WITHHELD};
use crate::chat::gate::{
    Author, ChannelDirectory, ChannelInfo, IncomingMessage, PilotSettings, SEEN_REACTION,
};
use crate::chat::persona::{CompiledPersona, PersonaId, PersonaRoot};
use crate::chat::sanitize::schedule_defaults;
use crate::chat::tools::ToolContext;
use crate::domain::members::Roster;
use crate::domain::model_log::{ChatInteraction, ChatOutcome};
use crate::infrastructure::llm::Message;
use crate::infrastructure::store::MemoryScheduleStore;

const GUILD: &str = "900";
const ROLE: &str = "10";
const CATEGORY: &str = "40";
const CHANNEL: &str = "50";
const THREAD: &str = "60";
const OTHER: &str = "70";
const BOT: &str = "800";

fn kanade() -> CompiledPersona {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas");
    let root = PersonaRoot::open(&root).expect("tracked personas");
    let id = PersonaId::parse("kanade").expect("persona id");
    CompiledPersona::compile(&root.load_bundle(&id).expect("bundle").value, None)
}

struct Channels(HashMap<&'static str, ChannelInfo>);

impl ChannelDirectory for Channels {
    fn channel(&self, id: &str) -> Option<ChannelInfo> {
        self.0.get(id).cloned()
    }
}

fn channels() -> Channels {
    let info = |id: &str, category: Option<&str>, parent: Option<&str>| ChannelInfo {
        id: id.into(),
        name: None,
        category_id: category.map(Into::into),
        parent_id: parent.map(Into::into),
    };
    Channels(HashMap::from([
        (CHANNEL, info(CHANNEL, Some(CATEGORY), None)),
        (THREAD, info(THREAD, None, Some(CHANNEL))),
        (OTHER, info(OTHER, Some(CATEGORY), None)),
    ]))
}

/// What one `answer` call does.
enum Step {
    Reply(&'static str),
    /// Reply once notified.
    Held(Arc<Notify>, &'static str),
    /// Never answers (only a shutdown cut ends it).
    Forever,
    Panic,
}

#[derive(Default)]
struct Seen {
    conversations: Vec<Vec<Message>>,
    clean_retry: Vec<bool>,
    ctx: Vec<ToolContext>,
}

struct Fake {
    enabled: AtomicBool,
    member_rate: (usize, f64),
    channels: Channels,
    steps: Mutex<VecDeque<Step>>,
    seen: Mutex<Seen>,
    rows: Mutex<Vec<ChatInteraction>>,
    /// `prepare` waits for this once, when set.
    prepare_gate: Mutex<Option<Arc<Notify>>>,
    /// Observed lifecycle events, one short line each.
    observed: Mutex<Vec<String>>,
}

impl Fake {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            member_rate: (4, 300.0),
            channels: channels(),
            steps: Mutex::new(steps.into()),
            seen: Mutex::default(),
            rows: Mutex::default(),
            prepare_gate: Mutex::default(),
            observed: Mutex::default(),
        }
    }
}

fn when() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 9, 4, 0, 0).unwrap()
}

impl Answerer for Arc<Fake> {
    fn setup(&self) -> Setup {
        Setup {
            enabled: self.enabled.load(Ordering::SeqCst),
            ready: true,
            pilot: PilotSettings {
                guild_id: GUILD.into(),
                channel_ids: Vec::new(),
                category_ids: vec![CATEGORY.into()],
                role_id: Some(ROLE.into()),
            },
            member_rate: self.member_rate,
            pool_rate: (12, 900.0),
            model: "chat-model".into(),
            now: when(),
        }
    }

    fn channels(&self) -> &(dyn ChannelDirectory + Send + Sync) {
        &self.channels
    }

    async fn prepare(&self, _asked: &Asked) -> Option<Prepared> {
        let gate = self.prepare_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.notified().await;
        }
        Some(Prepared {
            persona: kanade(),
            persona_key: "kanade".into(),
            directory: Arc::new(Roster::new()),
            members: Vec::new(),
            pilot: self.setup().pilot,
            model: "chat-model".into(),
            reasoning: None,
            now: when(),
            zone: chrono_tz::Asia::Kuala_Lumpur,
            reset: (Weekday::Thu, NaiveTime::MIN),
            bot_names: vec!["Kanade".into()],
        })
    }

    async fn answer(&self, job: Job<'_>) -> Generation {
        {
            let mut seen = self.seen.lock().unwrap();
            seen.conversations.push(job.question.conversation.clone());
            seen.clean_retry.push(job.question.settings.clean_retry);
            seen.ctx.push(job.question.ctx.clone());
        }
        let step = self.steps.lock().unwrap().pop_front();
        let reply = match step {
            Some(Step::Reply(text)) => text,
            Some(Step::Held(notify, text)) => {
                notify.notified().await;
                text
            }
            Some(Step::Forever) | None => std::future::pending().await,
            Some(Step::Panic) => panic!("scripted answerer panic"),
        };
        Generation {
            reply: reply.into(),
            ..Generation::default()
        }
    }

    async fn record(&self, row: ChatInteraction) {
        self.rows.lock().unwrap().push(row);
    }

    fn storm(&self, _alert: &crate::chat::pilot::StormAlert) {}

    fn observe(&self, event: &ChatEvent<'_>) {
        let line = match event {
            ChatEvent::Admitted { position, .. } => format!("admitted {position:?}"),
            ChatEvent::Ignored { reason } => format!("ignored {reason}"),
            ChatEvent::Finished {
                interaction,
                persona,
                model,
                ..
            } => format!(
                "finished {} {} {model} {}",
                interaction.outcome, persona.bundle, interaction.id
            ),
            ChatEvent::Cancelled { reason, .. } => format!("cancelled {reason}"),
            ChatEvent::SetupChanged { enabled, ready } => format!("setup {enabled} {ready}"),
        };
        self.observed.lock().unwrap().push(line);
    }
}

#[derive(Default)]
struct Knobs {
    /// Keycap adds take this long (a slow Discord).
    slow_keycap: Mutex<Option<Duration>>,
    /// Every removal hangs (a stuck Discord).
    hang_unreact: AtomicBool,
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>, Arc<Knobs>);

impl Recorder {
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl Surface for Recorder {
    async fn react(&self, channel_id: &str, message_id: &str, emoji: &str) {
        let slow = *self.1.slow_keycap.lock().unwrap();
        if let Some(delay) = slow.filter(|_| emoji.contains('\u{20e3}')) {
            tokio::time::sleep(delay).await;
        }
        self.0
            .lock()
            .unwrap()
            .push(format!("+{emoji} {channel_id}/{message_id}"));
    }

    async fn unreact(&self, channel_id: &str, message_id: &str, emoji: &str) {
        if self.1.hang_unreact.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        self.0
            .lock()
            .unwrap()
            .push(format!("-{emoji} {channel_id}/{message_id}"));
    }

    async fn reply(&self, channel_id: &str, reply_to: &str, text: &str) -> Result<String, String> {
        let mut events = self.0.lock().unwrap();
        events.push(format!("reply {channel_id}/{reply_to}: {text}"));
        Ok(format!("9{}", events.len()))
    }
}

type Clock = Arc<Mutex<f64>>;

struct Rig {
    driver: ChatDriver<Arc<Fake>, Recorder>,
    fake: Arc<Fake>,
    surface: Recorder,
    clock: Clock,
}

async fn rig_with(fake: Fake, config: DriverConfig, log: &MemoryScheduleStore) -> Rig {
    let fake = Arc::new(fake);
    let surface = Recorder::default();
    let clock: Clock = Arc::new(Mutex::new(10.0));
    let reading = Arc::clone(&clock);
    let driver = ChatDriver::start(
        config,
        Arc::clone(&fake),
        surface.clone(),
        log,
        Arc::new(move || *reading.lock().unwrap()),
    )
    .await
    .expect("driver starts");
    Rig {
        driver,
        fake,
        surface,
        clock,
    }
}

async fn rig(steps: Vec<Step>) -> Rig {
    rig_with(
        Fake::new(steps),
        DriverConfig::default(),
        &MemoryScheduleStore::new(),
    )
    .await
}

fn asked(id: &str, member: &str, channel: &str, roles: &[&str]) -> Asked {
    let origin = if channel == THREAD { CHANNEL } else { channel };
    Asked {
        message: QuestionMessage {
            id: id.into(),
            author_id: member.into(),
            content: format!("<@{BOT}> when is lotus?"),
            reference: None,
        },
        channel_id: channel.into(),
        origin_id: origin.into(),
        gate: IncomingMessage {
            author: Some(Author {
                id: member.into(),
                bot: false,
                roles: roles.iter().map(|role| (*role).to_owned()).collect(),
            }),
            guild_id: Some(GUILD.into()),
            channel: channels().channel(channel),
            mentions: vec![BOT.into()],
            role_mentions: Vec::new(),
        },
        replied_author_id: None,
        bot_user_id: Some(BOT.into()),
        self_role_id: None,
        is_admin: false,
    }
}

impl Rig {
    async fn settle(&self) {
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    fn rows(&self) -> Vec<ChatInteraction> {
        self.fake.rows.lock().unwrap().clone()
    }

    fn pool_used(&self) -> usize {
        self.driver.limits().allowance.pool.used
    }

    fn advance(&self, seconds: f64) {
        *self.clock.lock().unwrap() += seconds;
    }
}

#[tokio::test]
async fn a_disabled_chat_ignores_everything() {
    let rig = rig(vec![Step::Reply("hi")]).await;
    rig.fake.enabled.store(false, Ordering::SeqCst);
    assert!(!rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.surface.events().is_empty());
    assert!(rig.rows().is_empty());
    assert_eq!(rig.driver.status(), "disabled");
}

#[tokio::test]
async fn a_thread_question_in_a_chat_category_is_answered_as_a_reply_and_logged() {
    let rig = rig(vec![Step::Reply("Lotus is at 9.")]).await;
    assert!(rig.driver.offer(asked("1001", "11", THREAD, &[ROLE])));
    rig.settle().await;
    assert_eq!(
        rig.surface.events(),
        [
            format!("+{SEEN_REACTION} {THREAD}/1001"),
            format!("reply {THREAD}/1001: Lotus is at 9."),
            format!("-{SEEN_REACTION} {THREAD}/1001"),
        ]
    );
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.outcome, ChatOutcome::Answered);
    assert_eq!(row.reply, "Lotus is at 9.");
    // Queue, history and cards key on the parent channel.
    assert_eq!(row.channel_id.as_deref(), Some(CHANNEL));
    assert_eq!(row.message_id.as_deref(), Some("1001"));
    assert_eq!(rig.driver.status(), "idle");
}

#[tokio::test]
async fn members_without_the_pilot_role_are_gated_but_staff_are_not() {
    let rig = rig(vec![Step::Reply("Hello staff.")]).await;
    assert!(!rig.driver.offer(asked("1001", "11", CHANNEL, &[])));
    rig.settle().await;
    assert!(rig.surface.events().is_empty());
    assert_eq!(rig.pool_used(), 0);

    let mut staff = asked("1002", "12", CHANNEL, &[]);
    staff.is_admin = true;
    assert!(rig.driver.offer(staff));
    rig.settle().await;
    assert_eq!(rig.rows().len(), 1);
    assert_eq!(rig.pool_used(), 0, "staff spend no allowance");
}

#[tokio::test]
async fn a_message_outside_the_chat_categories_is_not_taken() {
    let rig = rig(vec![Step::Reply("x")]).await;
    let mut elsewhere = asked("1001", "11", "99", &[ROLE]);
    elsewhere.gate.channel = Some(ChannelInfo::bare("99"));
    assert!(!rig.driver.offer(elsewhere));
}

#[tokio::test]
async fn queued_questions_show_their_position_and_a_deleted_one_is_refunded() {
    let first = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&first), "first answer")]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1003", "13", CHANNEL, &[ROLE])));
    rig.settle().await;
    let events = rig.surface.events();
    assert!(events.contains(&format!("+{} {CHANNEL}/1002", position_reaction(1))));
    assert!(events.contains(&format!("+{} {CHANNEL}/1003", position_reaction(2))));
    assert_eq!(rig.pool_used(), 3);
    assert_eq!(rig.driver.status(), "busy");

    rig.driver.deleted(&["1002".into()]);
    assert_eq!(rig.pool_used(), 2, "the deleted waiter is refunded");
    assert_eq!(rig.driver.limits().queue.waiting.len(), 1);

    first.notify_one();
    rig.settle().await;
    // 1003 runs next (its keycap comes off); the deleted one never does.
    let events = rig.surface.events();
    assert!(events.contains(&format!("-{} {CHANNEL}/1003", position_reaction(2))));
    assert!(!events.iter().any(|event| event.contains("/1002: ")));
    assert_eq!(rig.fake.seen.lock().unwrap().conversations.len(), 2);
}

#[tokio::test]
async fn a_full_queue_sheds_with_a_busy_reaction_and_a_refund() {
    let config = DriverConfig {
        traffic: TrafficLimits {
            per_channel: 1,
            guild: 10,
            max_wait_s: 120.0,
        },
        ..DriverConfig::default()
    };
    let held = Arc::new(Notify::new());
    let rig = rig_with(
        Fake::new(vec![Step::Held(Arc::clone(&held), "a")]),
        config,
        &MemoryScheduleStore::new(),
    )
    .await;
    for (id, member) in [("1001", "11"), ("1002", "12"), ("1003", "13")] {
        assert!(rig.driver.offer(asked(id, member, CHANNEL, &[ROLE])));
    }
    rig.settle().await;
    assert!(
        rig.surface
            .events()
            .contains(&format!("+{CHANNEL_BUSY_REACTION} {CHANNEL}/1003"))
    );
    assert_eq!(rig.pool_used(), 2);
    held.notify_one();
}

#[tokio::test]
async fn a_waiter_past_its_bound_is_given_up_with_a_refund() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "a")]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.advance(121.0);
    held.notify_one();
    rig.settle().await;
    let events = rig.surface.events();
    assert!(events.contains(&format!("+{CHANNEL_BUSY_REACTION} {CHANNEL}/1002")));
    assert_eq!(rig.driver.limits().allowance.pool.used, 1);
}

#[tokio::test]
async fn a_deleted_running_question_finishes_but_posts_nothing() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![
        Step::Held(Arc::clone(&held), "late answer"),
        Step::Reply("next"),
    ])
    .await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.deleted(&["1001".into()]);
    held.notify_one();
    rig.settle().await;
    assert!(!rig.surface.events().iter().any(|e| e.starts_with("reply")));
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].error.as_deref(),
        Some("cancelled: the question was deleted")
    );
    assert_eq!(rig.driver.limits().clean_retry.pending, 0, "concluded");

    // Neither the deleted question nor its unposted answer reaches history.
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    let later = prompt(&rig.fake.seen.lock().unwrap().conversations[1]);
    assert!(!later.contains("late answer"), "{later}");
    assert_eq!(later.matches(WITHHELD).count(), 2, "{later}");
}

#[tokio::test]
async fn a_refused_reservation_never_frees_a_running_one() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![
        Step::Held(Arc::clone(&held), "first"),
        Step::Reply("second"),
    ])
    .await;
    // One member, two channels: both run at once.
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.driver.offer(asked("1002", "11", OTHER, &[ROLE])));
    rig.settle().await;
    assert_eq!(
        rig.fake.seen.lock().unwrap().clean_retry,
        [true, false],
        "reserved at dequeue, one per member"
    );
    assert_eq!(rig.rows().len(), 1, "the second concluded");
    assert_eq!(
        rig.driver.limits().clean_retry.pending,
        1,
        "the refused question kept the running one's reservation"
    );
    held.notify_one();
    rig.settle().await;
    assert_eq!(rig.driver.limits().clean_retry.pending, 0);
}

#[tokio::test]
async fn a_spent_allowance_reacts_says_so_once_and_logs_rate_limited() {
    let mut fake = Fake::new(vec![Step::Reply("one")]);
    fake.member_rate = (1, 300.0);
    let rig = rig_with(fake, DriverConfig::default(), &MemoryScheduleStore::new()).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.driver.offer(asked("1002", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1003", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    let events = rig.surface.events();
    assert!(events.contains(&format!("+{RATE_LIMITED_REACTION} {CHANNEL}/1002")));
    let limited: Vec<_> = events
        .iter()
        .filter(|event| event.contains("That's your 1 answer"))
        .collect();
    assert_eq!(limited.len(), 1, "once per episode");
    let rows = rig.rows();
    let outcomes: Vec<_> = rows.iter().map(|row| row.outcome).collect();
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == ChatOutcome::RateLimited)
            .count(),
        2
    );
}

fn withheld_row(message_id: &str) -> ChatInteraction {
    ChatInteraction {
        id: "old-row".into(),
        at: when(),
        channel_id: Some(CHANNEL.into()),
        message_id: Some(message_id.into()),
        member_id: Some("11".into()),
        question: "something filtered".into(),
        reply: "Sorry".into(),
        outcome: ChatOutcome::ContentBlocked,
        error: None,
        clean_retry: false,
        withheld: true,
        guardrail: json!({"content_filter": true}),
        request_count: 1,
        latency_ms: None,
        model_ms: None,
        tools_ms: None,
        prompt_tokens: None,
        completion_tokens: None,
        rounds: Vec::new(),
    }
}

#[tokio::test]
async fn withheld_questions_are_reloaded_before_the_first_admission() {
    let log = MemoryScheduleStore::new();
    log.record_chat(withheld_row("700")).await.unwrap();
    let rig = rig_with(
        Fake::new(vec![Step::Reply("ok")]),
        DriverConfig::default(),
        &log,
    )
    .await;
    let mut reply = asked("1001", "11", CHANNEL, &[ROLE]);
    reply.message.reference = Some(Reference {
        message_id: Some("700".into()),
        resolved: Some(Box::new(Parent {
            id: "700".into(),
            author_id: Some("12".into()),
            content: Some("the filtered secret".into()),
            reference: None,
        })),
    });
    assert!(rig.driver.offer(reply));
    rig.settle().await;
    let seen = rig.fake.seen.lock().unwrap();
    let prompt = prompt(&seen.conversations[0]);
    assert!(prompt.contains(WITHHELD));
    assert!(!prompt.contains("the filtered secret"));
}

#[tokio::test]
async fn the_question_timeout_must_stay_below_the_clean_retry_window() {
    let too_long = DriverConfig {
        timeout: Duration::from_secs(600),
        ..DriverConfig::default()
    };
    assert!(too_long.validate().is_err());
    assert!(DriverConfig::default().validate().is_ok());
    assert!(DriverConfig::default().timeout < Duration::from_secs(600));
    let started = ChatDriver::start(
        too_long,
        Arc::new(Fake::new(Vec::new())),
        Recorder::default(),
        &MemoryScheduleStore::new(),
        Arc::new(|| 0.0),
    )
    .await;
    assert!(matches!(started, Err(DriverError::Config(_))));
}

#[tokio::test]
async fn shutdown_cuts_running_questions_refunds_and_concludes_them() {
    let config = DriverConfig {
        stop_grace: Duration::from_millis(50),
        ..DriverConfig::default()
    };
    let rig = rig_with(
        Fake::new(vec![Step::Forever]),
        config,
        &MemoryScheduleStore::new(),
    )
    .await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(rig.pool_used(), 2);

    rig.driver.stop().await;
    assert_eq!(rig.pool_used(), 0, "both refunded");
    let rows = rig.rows();
    assert_eq!(
        rows.len(),
        1,
        "the running one concluded; the waiter never ran"
    );
    assert_eq!(rows[0].error.as_deref(), Some("cancelled: serve shut down"));
    assert_eq!(rig.driver.limits().clean_retry.pending, 0);
    let events = rig.surface.events();
    assert!(!events.iter().any(|event| event.starts_with("reply")));
    assert!(events.contains(&format!("-{} {CHANNEL}/1002", position_reaction(1))));
    assert!(
        !rig.driver.offer(asked("1003", "13", CHANNEL, &[ROLE])),
        "closed"
    );
}

#[tokio::test]
async fn a_zero_member_allowance_ignores_members_silently_but_not_staff_or_overrides() {
    let mut fake = Fake::new(vec![Step::Reply("Hello staff."), Step::Reply("Hi.")]);
    fake.member_rate = (0, 300.0);
    let rig = rig_with(fake, DriverConfig::default(), &MemoryScheduleStore::new()).await;
    for id in ["1001", "1002"] {
        assert!(!rig.driver.offer(asked(id, "11", CHANNEL, &[ROLE])));
    }
    rig.settle().await;
    assert!(rig.surface.events().is_empty(), "no reaction or reply");
    assert!(rig.rows().is_empty(), "no log row");
    assert_eq!(rig.pool_used(), 0);

    let mut staff = asked("1003", "12", CHANNEL, &[]);
    staff.is_admin = true;
    assert!(rig.driver.offer(staff));
    rig.settle().await;
    rig.driver
        .set_overrides(vec![crate::domain::model_log::AllowanceOverride {
            member_id: "13".into(),
            count: 2,
            window_ms: 300_000,
            updated_at: when(),
        }]);
    assert!(rig.driver.offer(asked("1004", "13", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(rig.rows().len(), 2, "staff and the override are answered");
    assert_eq!(rig.pool_used(), 1, "only the override spent");
}

/// Every text the model was sent, one message per line.
fn prompt(conversation: &[Message]) -> String {
    conversation
        .iter()
        .filter_map(|message| match message {
            Message::System { content } | Message::User { content } => Some(content.clone()),
            Message::Assistant { content, .. } => content.clone(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const SELF_ROLE: &str = "555";

#[tokio::test]
async fn the_bots_managed_role_mention_is_answered_and_stripped() {
    let rig = rig(vec![Step::Reply("Here is the week.")]).await;
    let mut by_role = asked("1001", "11", CHANNEL, &[ROLE]);
    by_role.message.content = format!("<@&{SELF_ROLE}> what's on this week?");
    by_role.gate.mentions.clear();
    by_role.gate.role_mentions = vec![SELF_ROLE.into()];
    let mut unknown_role = by_role.clone();
    unknown_role.message.id = "1002".into();
    // Without the managed role known, a role mention does not summon.
    assert!(!rig.driver.offer(unknown_role));
    by_role.self_role_id = Some(SELF_ROLE.into());
    assert!(rig.driver.offer(by_role.clone()));
    rig.settle().await;
    assert_eq!(rig.rows()[0].outcome, ChatOutcome::Answered);
    let seen = rig.fake.seen.lock().unwrap();
    let ctx = &seen.ctx[0];
    assert_eq!(ctx.self_role_id.as_deref(), Some(SELF_ROLE));
    assert_eq!(ctx.bot_names, ["Kanade"]);
    let stripped = schedule_defaults(&by_role.message.content, Some(BOT), Some(SELF_ROLE));
    let kept = schedule_defaults(&by_role.message.content, Some(BOT), None);
    assert_ne!(stripped, kept, "the role mention changes the reading");
    assert_eq!(
        (ctx.force_all_channels, ctx.force_group_schedule),
        (stripped.force_all_channels, stripped.force_group_schedule)
    );
}

#[tokio::test]
async fn a_question_deleted_before_its_answer_starts_never_reaches_the_model() {
    let rig = rig(vec![Step::Reply("never")]).await;
    let gate = Arc::new(Notify::new());
    *rig.fake.prepare_gate.lock().unwrap() = Some(Arc::clone(&gate));
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(rig.pool_used(), 1);
    rig.driver.deleted(&["1001".into()]);
    gate.notify_one();
    rig.settle().await;
    assert!(rig.fake.seen.lock().unwrap().conversations.is_empty());
    assert!(rig.rows().is_empty());
    assert_eq!(rig.pool_used(), 0, "refunded");
    assert_eq!(rig.driver.limits().clean_retry.pending, 0);
    assert_eq!(
        rig.surface.events().last().map(String::as_str),
        Some(format!("-{SEEN_REACTION} {CHANNEL}/1001").as_str())
    );
    assert_eq!(rig.driver.status(), "idle");
}

#[tokio::test]
async fn waiters_are_dropped_and_refunded_when_chat_is_turned_off() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "first")]).await;
    for (id, member) in [("1001", "11"), ("1002", "12"), ("1003", "13")] {
        assert!(rig.driver.offer(asked(id, member, CHANNEL, &[ROLE])));
    }
    rig.settle().await;
    assert_eq!(rig.pool_used(), 3);
    rig.fake.enabled.store(false, Ordering::SeqCst);
    held.notify_one();
    rig.settle().await;
    assert_eq!(rig.fake.seen.lock().unwrap().conversations.len(), 1);
    assert_eq!(rig.rows().len(), 1);
    assert_eq!(rig.pool_used(), 1, "only the answered one is spent");
    let events = rig.surface.events();
    for (id, position) in [("1002", 1), ("1003", 2)] {
        assert!(events.contains(&format!("-{} {CHANNEL}/{id}", position_reaction(position))));
        assert!(!events.contains(&format!("+{SEEN_REACTION} {CHANNEL}/{id}")));
    }
    assert!(rig.driver.limits().queue.answering.is_empty());
}

#[tokio::test]
async fn a_panicking_answer_concludes_and_frees_the_channel() {
    let rig = rig(vec![Step::Panic, Step::Reply("after")]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].error.as_deref(),
        Some("failed: the question stopped unexpectedly")
    );
    let limits = rig.driver.limits();
    assert_eq!(limits.clean_retry.pending, 0, "the reservation settled");
    assert!(limits.queue.answering.is_empty(), "the channel is free");
    assert_eq!(limits.allowance.pool.used, 0, "refunded");
    assert!(!rig.surface.events().iter().any(|e| e.starts_with("reply")));
    assert!(rig.driver.offer(asked("1002", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(
        rig.surface
            .events()
            .contains(&format!("reply {CHANNEL}/1002: after"))
    );
}

#[tokio::test]
async fn a_keycap_is_removed_only_after_it_was_added() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "a"), Step::Reply("b")]).await;
    *rig.surface.1.slow_keycap.lock().unwrap() = Some(Duration::from_millis(100));
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    // Dequeued at once, while the add is still in flight.
    held.notify_one();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let events = rig.surface.events();
    let keycap = position_reaction(1);
    let added = events
        .iter()
        .position(|e| *e == format!("+{keycap} {CHANNEL}/1002"));
    let removed = events
        .iter()
        .position(|e| *e == format!("-{keycap} {CHANNEL}/1002"));
    assert!(added.is_some() && removed.is_some(), "{events:?}");
    assert!(added < removed, "{events:?}");
}

#[tokio::test]
async fn shutdown_is_bounded_even_when_discord_hangs() {
    let config = DriverConfig {
        stop_grace: Duration::from_millis(50),
        cut_budget: Duration::from_millis(50),
        ..DriverConfig::default()
    };
    let rig = rig_with(
        Fake::new(vec![Step::Forever]),
        config,
        &MemoryScheduleStore::new(),
    )
    .await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.surface.1.hang_unreact.store(true, Ordering::SeqCst);
    let started = tokio::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(3), rig.driver.stop())
        .await
        .expect("stop is bounded");
    // grace + cut budget + the final log budget (1 s), never the hang.
    assert!(started.elapsed() < Duration::from_millis(1500));
    let rows = rig.rows();
    assert_eq!(rows.len(), 1, "logged before the hung tidy-up");
    assert_eq!(rows[0].error.as_deref(), Some("cancelled: serve shut down"));
    assert_eq!(rig.pool_used(), 0);
    assert_eq!(rig.driver.limits().clean_retry.pending, 0);
}

#[tokio::test]
async fn lifecycle_events_cover_summons_only_and_link_the_log_row() {
    let rig = rig(vec![Step::Reply("hi")]).await;
    let mut unmentioned = asked("1001", "11", CHANNEL, &[ROLE]);
    unmentioned.gate.mentions.clear();
    assert!(!rig.driver.offer(unmentioned));
    assert!(!rig.driver.offer(asked("1002", "12", CHANNEL, &[])));
    assert!(rig.driver.offer(asked("1003", "13", CHANNEL, &[ROLE])));
    rig.settle().await;
    let observed = rig.fake.observed.lock().unwrap().clone();
    let row = rig.rows().pop().expect("row");
    assert_eq!(
        observed,
        vec![
            "setup true true".to_owned(),
            "ignored no_pilot_role".to_owned(),
            "admitted None".to_owned(),
            format!("finished answered kanade chat-model {}", row.id),
        ]
    );
}

#[tokio::test]
async fn a_disabled_chat_logs_ignored_only_for_summons() {
    let rig = rig(Vec::new()).await;
    rig.fake.enabled.store(false, Ordering::SeqCst);
    let mut chatter = asked("1001", "11", CHANNEL, &[ROLE]);
    chatter.gate.mentions.clear();
    rig.driver.offer(chatter);
    rig.driver.offer(asked("1002", "11", CHANNEL, &[ROLE]));
    let observed = rig.fake.observed.lock().unwrap().clone();
    assert_eq!(observed, ["setup false true", "ignored disabled"]);
}
