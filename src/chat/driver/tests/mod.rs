//! The driver over a scripted answerer and a recording surface whose
//! message effects go through `DiscordSurface` to a `FakeDiscord` (scripted
//! outcomes and hold barriers); the monotonic clock is the test's. The
//! delivery matrix (T1–T28) is in `delivery.rs`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{NaiveTime, TimeZone, Utc, Weekday};
use serde_json::json;
use tokio::sync::Notify;

use super::*;
use crate::bot::chat_feed::DiscordSurface;
use crate::bot::mentions;
use crate::bot::transport::{Call, FakeDiscord, Op};
use crate::chat::answer::Generation;
use crate::chat::context::{Parent, QuestionMessage, Reference, TurnRole, WITHHELD};
use crate::chat::gate::{
    Author, ChannelDirectory, ChannelInfo, IncomingMessage, PilotSettings, SEEN_REACTION,
};
use crate::chat::persona::{CompiledPersona, PersonaId, PersonaRoot};
use crate::chat::sanitize::{FAILURE_REPLY, schedule_defaults};
use crate::chat::tools::ToolContext;
use crate::domain::catalog::BossTable;
use crate::domain::members::{Directory, Member, Roster};
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

fn catalog() -> Arc<BossTable> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("boss/bosses.yaml");
    Arc::new(crate::infrastructure::files::load_catalog(&path).expect("shipped catalog"))
}

/// The tracked persona's staging line for the test question ("when is").
fn staged() -> String {
    kanade().staging_lines().schedule.clone()
}

/// Live like the guild cache: a channel may move category meanwhile.
struct Channels(Mutex<HashMap<&'static str, ChannelInfo>>);

impl ChannelDirectory for Channels {
    fn channel(&self, id: &str) -> Option<ChannelInfo> {
        self.0.lock().unwrap().get(id).cloned()
    }
}

impl Channels {
    fn move_to(&self, id: &str, category: &str) {
        let mut channels = self.0.lock().unwrap();
        let channel = channels.get_mut(id).expect("known channel");
        channel.category_id = Some(category.into());
    }
}

/// A member directory whose first lookup panics (panic injection while
/// the question's context is built under the state lock).
struct PanicOnce(AtomicBool, Roster);

impl Directory for PanicOnce {
    fn member(&self, user_id: &str) -> Option<Member> {
        assert!(
            !self.0.swap(false, Ordering::SeqCst),
            "injected directory panic"
        );
        self.1.member(user_id)
    }

    fn is_watched(&self, channel_id: &str) -> bool {
        self.1.is_watched(channel_id)
    }
}

/// A directory whose every lookup panics, so concluding panics too.
struct PanicAlways;

impl Directory for PanicAlways {
    fn member(&self, _user_id: &str) -> Option<Member> {
        panic!("injected permanent directory panic")
    }

    fn is_watched(&self, _channel_id: &str) -> bool {
        true
    }
}

fn channels() -> Channels {
    let info = |id: &str, category: Option<&str>, parent: Option<&str>| ChannelInfo {
        id: id.into(),
        name: None,
        category_id: category.map(Into::into),
        parent_id: parent.map(Into::into),
    };
    Channels(Mutex::new(HashMap::from([
        (CHANNEL, info(CHANNEL, Some(CATEGORY), None)),
        (THREAD, info(THREAD, None, Some(CHANNEL))),
        (OTHER, info(OTHER, Some(CATEGORY), None)),
    ])))
}

/// What one `answer` call does.
enum Step {
    Reply(&'static str),
    /// Reply once notified.
    Held(Arc<Notify>, &'static str),
    /// Never answers (only a shutdown cut ends it).
    Forever,
    Panic,
    /// Panic once notified.
    PanicAfter(Arc<Notify>),
}

#[derive(Default)]
struct Seen {
    conversations: Vec<Vec<Message>>,
    clean_retry: Vec<bool>,
    ctx: Vec<ToolContext>,
}

struct Fake {
    enabled: AtomicBool,
    owns_rejection: AtomicBool,
    member_rate: (usize, f64),
    channels: Channels,
    steps: Mutex<VecDeque<Step>>,
    seen: Mutex<Seen>,
    rows: Mutex<Vec<ChatInteraction>>,
    /// `prepare` waits for this once, when set.
    prepare_gate: Mutex<Option<Arc<Notify>>>,
    /// Observed lifecycle events, one short line each.
    observed: Mutex<Vec<String>>,
    /// The next prepared directory panics on its first lookup.
    panic_directory: AtomicBool,
    panic_directory_always: AtomicBool,
}

impl Fake {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            enabled: AtomicBool::new(true),
            owns_rejection: AtomicBool::new(true),
            member_rate: (4, 300.0),
            channels: channels(),
            steps: Mutex::new(steps.into()),
            seen: Mutex::default(),
            rows: Mutex::default(),
            prepare_gate: Mutex::default(),
            observed: Mutex::default(),
            panic_directory: AtomicBool::new(false),
            panic_directory_always: AtomicBool::new(false),
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
            catalog: catalog(),
            persona_key: "kanade".into(),
            directory: if self.panic_directory_always.swap(false, Ordering::SeqCst) {
                Arc::new(PanicAlways)
            } else if self.panic_directory.swap(false, Ordering::SeqCst) {
                Arc::new(PanicOnce(AtomicBool::new(true), Roster::new()))
            } else {
                Arc::new(Roster::new())
            },
            members: Vec::new(),
            pilot: self.setup().pilot,
            model: "chat-model".into(),
            reasoning: None,
            context_window: DEFAULT_CONTEXT_TOKENS,
            max_output_tokens: crate::chat::context::COMPLETION_RESERVE_TOKENS as u32,
            context_source: "local_default",
            route: None,
            now: when(),
            zone: chrono_tz::Asia::Kuala_Lumpur,
            reset: (Weekday::Thu, NaiveTime::MIN),
            bot_names: vec!["Kanade".into()],
        })
    }

    async fn owns_rejection(&self, _request: &FollowUpRequest) -> bool {
        self.owns_rejection.load(Ordering::SeqCst)
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
            Some(Step::PanicAfter(notify)) => {
                notify.notified().await;
                panic!("scripted answerer panic")
            }
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
    /// The n-th post (1-based) panics before reaching Discord; 0 never.
    panic_on_post: AtomicUsize,
    posts: AtomicUsize,
}

/// Reactions as event lines; message effects through `DiscordSurface` to
/// the fake Discord (message ids from 5001).
#[derive(Clone)]
struct Recorder(Arc<Mutex<Vec<String>>>, Arc<Knobs>, Arc<FakeDiscord>);

impl Default for Recorder {
    fn default() -> Self {
        Self(
            Arc::default(),
            Arc::default(),
            Arc::new(FakeDiscord::with_first_message_id(5001)),
        )
    }
}

impl Recorder {
    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    fn discord(&self) -> DiscordSurface<FakeDiscord> {
        DiscordSurface(Arc::clone(&self.2))
    }
}

/// Every create and edit Discord saw (delivered or not), typing excluded:
/// `post[ silent][ ^reply_to] <channel>: text`, `edit <id>: text`,
/// `delete <id>`. Each must mention nobody (T27).
fn effects(fake: &FakeDiscord) -> Vec<String> {
    let mut flags = fake.create_flags().into_iter();
    fake.calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Create {
                channel, message, ..
            } => {
                assert_eq!(message.allowed_mentions, mentions::none());
                let silent = flags
                    .next()
                    .expect("flags")
                    .contains(crate::bot::transport::SILENT);
                let to = message
                    .reply_to
                    .map(|id| format!(" ^{id}"))
                    .unwrap_or_default();
                let silent = if silent { " silent" } else { "" };
                Some(format!(
                    "post{silent}{to} {channel}: {}",
                    message.content.unwrap_or_default()
                ))
            }
            Call::Edit { message, edit, .. } => {
                assert_eq!(edit.allowed_mentions, mentions::none());
                assert!(edit.embeds.is_none());
                Some(format!(
                    "edit {message}: {}",
                    edit.content.unwrap_or_default()
                ))
            }
            Call::Delete { message, .. } => Some(format!("delete {message}")),
            _ => None,
        })
        .collect()
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

    async fn post(&self, channel_id: &str, post: Post<'_>) -> Effect<String> {
        let n = self.1.posts.fetch_add(1, Ordering::SeqCst) + 1;
        assert!(
            self.1.panic_on_post.load(Ordering::SeqCst) != n,
            "injected surface panic"
        );
        self.discord().post(channel_id, post).await
    }

    async fn edit(&self, channel_id: &str, message_id: &str, text: &str) -> Effect<()> {
        self.discord().edit(channel_id, message_id, text).await
    }

    async fn delete(&self, channel_id: &str, message_id: &str) -> Effect<()> {
        self.discord().delete(channel_id, message_id).await
    }

    async fn typing(&self, channel_id: &str) {
        self.discord().typing(channel_id).await;
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

    fn fake_discord(&self) -> &FakeDiscord {
        &self.surface.2
    }

    fn effects(&self) -> Vec<String> {
        effects(self.fake_discord())
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
            format!("-{SEEN_REACTION} {THREAD}/1001"),
        ]
    );
    assert_eq!(
        rig.effects(),
        [
            format!("post silent ^1001 {THREAD}: {}", staged()),
            "edit 5001: Lotus is at 9.".to_owned(),
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
    assert!(!rig.effects().iter().any(|effect| effect.contains("^1002")));
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
    assert_eq!(
        rig.effects(),
        [
            format!("post silent ^1001 {CHANNEL}: {}", staged()),
            "delete 5001".to_owned(),
        ],
        "the placeholder is withdrawn; the late answer never posts"
    );
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
    let effects = rig.effects();
    let limited: Vec<_> = effects
        .iter()
        .filter(|effect| effect.contains("That's your 1 answer"))
        .collect();
    assert!(
        limited[0].starts_with(&format!("post ^1002 {CHANNEL}: ")),
        "an audible reply, as before: {limited:?}"
    );
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
        persona: None,
        profile: None,
        profile_source: None,
        error_code: None,
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
    assert_eq!(
        rig.effects(),
        [
            format!("post silent ^1001 {CHANNEL}: {}", staged()),
            format!("edit 5001: {FAILURE_REPLY}"),
        ],
        "D5: the placeholder becomes the failure line"
    );
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
async fn a_first_person_schedule_signal_comes_from_the_unmasked_question() {
    let rig = rig(vec![Step::Reply("Here are your runs.")]).await;
    let mut asked = asked("1001", "11", CHANNEL, &[ROLE]);
    asked.message.content = format!("<@{BOT}> what's for me today?");
    assert!(rig.driver.offer(asked));
    rig.settle().await;
    let seen = rig.fake.seen.lock().unwrap();
    assert!(seen.ctx[0].self_schedule_requested);
    assert!(!seen.ctx[0].force_group_schedule);
    assert!(
        !schedule_defaults("what's for someone else today?", Some(BOT), None)
            .self_schedule_requested
    );
    for mixed in [
        "what's for me and someone else today?",
        "my runs with a friend tonight?",
        "what's for me, a friend, today?",
        "what's for me and everyone?",
    ] {
        assert!(
            !schedule_defaults(mixed, Some(BOT), None).self_schedule_requested,
            "{mixed}"
        );
    }
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
    assert_eq!(
        rig.effects(),
        [
            format!("post silent ^1001 {CHANNEL}: {}", staged()),
            format!("edit 5001: {FAILURE_REPLY}"),
        ]
    );
    assert!(rig.driver.offer(asked("1002", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert!(rig.effects().contains(&"edit 5002: after".to_owned()));
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

#[tokio::test]
async fn a_panic_while_context_is_built_refunds_and_logs() {
    let rig = rig(vec![Step::Reply("after")]).await;
    rig.fake.panic_directory.store(true, Ordering::SeqCst);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    let limits = rig.driver.limits();
    assert_eq!(limits.clean_retry.pending, 0, "the reservation settled");
    assert_eq!(limits.allowance.pool.used, 0, "refunded");
    assert!(limits.queue.answering.is_empty(), "the channel is free");
    let rows = rig.rows();
    assert_eq!(rows.len(), 1, "concluded and logged");
    assert_eq!(
        rows[0].error.as_deref(),
        Some("failed: the question stopped unexpectedly")
    );
    assert!(rig.fake.seen.lock().unwrap().conversations.is_empty());
    assert!(
        rig.surface
            .events()
            .contains(&format!("-{SEEN_REACTION} {CHANNEL}/1001"))
    );

    assert!(rig.driver.offer(asked("1002", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(
        rig.fake.seen.lock().unwrap().clean_retry,
        [true],
        "the member's clean retry was not leaked"
    );
}

/// Concluding runs the lookup that panicked again; during unwinding that
/// second panic would abort the process, so it is deferred and, failing
/// again, the question is only settled and refunded.
#[tokio::test]
async fn a_lookup_that_always_panics_settles_without_aborting() {
    let rig = rig(vec![Step::Reply("after")]).await;
    rig.fake
        .panic_directory_always
        .store(true, Ordering::SeqCst);
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    let limits = rig.driver.limits();
    assert_eq!(limits.clean_retry.pending, 0, "the reservation settled");
    assert_eq!(limits.allowance.pool.used, 0, "refunded");
    assert!(limits.queue.answering.is_empty(), "the channel is free");
    assert!(rig.rows().is_empty(), "concluding could not run");
    assert!(
        rig.surface
            .events()
            .contains(&format!("-{SEEN_REACTION} {CHANNEL}/1001"))
    );

    assert!(rig.driver.offer(asked("1002", "11", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(rig.rows().len(), 1, "the next question is answered");
    assert_eq!(rig.fake.seen.lock().unwrap().clean_retry, [true]);
}

#[tokio::test]
async fn a_waiter_whose_channel_left_the_chat_category_is_not_answered() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![
        Step::Held(Arc::clone(&held), "first"),
        Step::Reply("x"),
    ])
    .await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    assert_eq!(rig.pool_used(), 2);
    // Moved out of the chat category while 1002 waited; its admission
    // snapshot still says otherwise.
    rig.fake.channels.move_to(CHANNEL, "41");
    held.notify_one();
    rig.settle().await;
    assert_eq!(rig.fake.seen.lock().unwrap().conversations.len(), 1);
    assert_eq!(rig.pool_used(), 1, "the waiter is refunded");
    assert!(
        rig.fake
            .observed
            .lock()
            .unwrap()
            .contains(&"cancelled not_admitted".to_owned())
    );
    let events = rig.surface.events();
    assert!(!events.contains(&format!("+{SEEN_REACTION} {CHANNEL}/1002")));
    assert!(events.contains(&format!("-{} {CHANNEL}/1002", position_reaction(1))));
}

/// Keycaps a message still shows, from the surface's add/remove log.
fn keycaps(events: &[String], message: &str) -> Vec<String> {
    let suffix = format!(" {CHANNEL}/{message}");
    let mut shown: Vec<String> = Vec::new();
    for event in events {
        let Some(head) = event.strip_suffix(&suffix) else {
            continue;
        };
        if !head.contains('\u{20e3}') && !head.contains('🔟') {
            continue;
        }
        if let Some(emoji) = head.strip_prefix('+') {
            shown.push(emoji.to_owned());
        } else if let Some(emoji) = head.strip_prefix('-') {
            shown.retain(|seen| seen != emoji);
        }
    }
    shown
}

#[tokio::test]
async fn queue_positions_are_renumbered_when_a_waiter_leaves() {
    let first = Arc::new(Notify::new());
    let second = Arc::new(Notify::new());
    let rig = rig(vec![
        Step::Held(Arc::clone(&first), "a"),
        Step::Held(Arc::clone(&second), "c"),
        Step::Reply("d"),
    ])
    .await;
    for (id, member) in [
        ("1001", "11"),
        ("1002", "12"),
        ("1003", "13"),
        ("1004", "14"),
    ] {
        assert!(rig.driver.offer(asked(id, member, CHANNEL, &[ROLE])));
    }
    rig.settle().await;
    let events = rig.surface.events();
    assert_eq!(keycaps(&events, "1003"), [position_reaction(2)]);
    assert_eq!(keycaps(&events, "1004"), [position_reaction(3)]);

    // A deleted waiter: everyone behind it moves up.
    rig.driver.deleted(&["1002".into()]);
    rig.settle().await;
    let events = rig.surface.events();
    assert_eq!(
        keycaps(&events, "1003"),
        [position_reaction(1)],
        "{events:?}"
    );
    assert_eq!(
        keycaps(&events, "1004"),
        [position_reaction(2)],
        "{events:?}"
    );

    // The head is dequeued: the rest move up again.
    first.notify_one();
    rig.settle().await;
    let events = rig.surface.events();
    assert!(keycaps(&events, "1003").is_empty(), "{events:?}");
    assert_eq!(
        keycaps(&events, "1004"),
        [position_reaction(1)],
        "{events:?}"
    );

    second.notify_one();
    rig.settle().await;
    let events = rig.surface.events();
    // 1002's message is gone, so its keycap is never touched again.
    for id in ["1003", "1004"] {
        assert!(keycaps(&events, id).is_empty(), "{id}: {events:?}");
    }
    assert_eq!(rig.rows().len(), 3);
}

#[tokio::test]
async fn an_expired_waiter_moves_the_rest_up() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "a"), Step::Forever]).await;
    assert!(rig.driver.offer(asked("1001", "11", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1002", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.advance(100.0);
    assert!(rig.driver.offer(asked("1003", "13", CHANNEL, &[ROLE])));
    assert!(rig.driver.offer(asked("1004", "14", CHANNEL, &[ROLE])));
    rig.settle().await;
    // 1002 outwaits its bound; 1003 runs, 1004 is first in line.
    rig.advance(30.0);
    held.notify_one();
    rig.settle().await;
    let events = rig.surface.events();
    assert!(events.contains(&format!("+{CHANNEL_BUSY_REACTION} {CHANNEL}/1002")));
    assert!(keycaps(&events, "1002").is_empty(), "{events:?}");
    assert!(keycaps(&events, "1003").is_empty(), "{events:?}");
    assert_eq!(
        keycaps(&events, "1004"),
        [position_reaction(1)],
        "{events:?}"
    );
}

fn rejected_card(id: &str, channel: &str) -> FollowUpRequest {
    FollowUpRequest {
        card_message_id: id.into(),
        channel_id: channel.into(),
        reactor_id: "11".into(),
        source_ids: vec!["chat-source".into()],
        cards: vec![FollowUpCard {
            summary: Some("move Lotus to Friday".into()),
            bosses: vec!["lotus".into()],
            participants: vec!["11".into()],
        }],
    }
}

#[tokio::test]
async fn a_rejected_chat_card_gets_one_read_only_generic_reply_and_remembers_only_its_answer() {
    let rig = rig(vec![Step::Reply(
        "The card is off. What time works instead?",
    )])
    .await;
    rig.driver.rejected(rejected_card("9001", CHANNEL)).await;
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [
            format!(
                "post silent ^9001 {CHANNEL}: {}",
                kanade().staging_lines().generic
            ),
            "edit 5001: The card is off. What time works instead?".to_owned(),
        ]
    );
    assert!(rig.fake.seen.lock().unwrap().ctx[0].read_only);
    assert_eq!(rig.pool_used(), 0, "follow-ups spend no allowance");
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].guardrail["kind"], "rejection_followup");
    let mut state = rig.driver.state();
    let history = state.pilot.conversations.history(CHANNEL, 10.0);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, TurnRole::Assistant);
    assert!(!history[0].content.contains("Note from the scheduler"));
}

#[tokio::test]
async fn rejected_cards_are_silent_when_the_scope_fails_or_the_channel_is_busy() {
    let held = Arc::new(Notify::new());
    let rig = rig(vec![Step::Held(Arc::clone(&held), "normal")]).await;
    rig.fake.owns_rejection.store(false, Ordering::SeqCst);
    rig.driver.rejected(rejected_card("9001", CHANNEL)).await;
    rig.fake.enabled.store(false, Ordering::SeqCst);
    rig.driver.rejected(rejected_card("9002", CHANNEL)).await;
    rig.fake.enabled.store(true, Ordering::SeqCst);
    rig.driver.rejected(rejected_card("9003", "99")).await;
    assert!(rig.effects().is_empty());

    rig.fake.owns_rejection.store(true, Ordering::SeqCst);
    assert!(rig.driver.offer(asked("1001", "12", CHANNEL, &[ROLE])));
    rig.settle().await;
    rig.driver.rejected(rejected_card("9004", CHANNEL)).await;
    rig.settle().await;
    assert!(!rig.effects().iter().any(|effect| effect.contains("^9004")));
    held.notify_one();
}

#[tokio::test]
async fn rejected_cards_share_a_thirty_second_channel_cooldown_and_shutdown_drains_them() {
    let held = Arc::new(Notify::new());
    let rig = rig_with(
        Fake::new(vec![Step::Held(Arc::clone(&held), "late")]),
        DriverConfig {
            stop_grace: Duration::from_millis(50),
            ..DriverConfig::default()
        },
        &MemoryScheduleStore::new(),
    )
    .await;
    rig.driver.rejected(rejected_card("9001", CHANNEL)).await;
    rig.driver.rejected(rejected_card("9002", CHANNEL)).await;
    rig.settle().await;
    assert_eq!(rig.fake.seen.lock().unwrap().conversations.len(), 1);
    rig.driver.stop().await;
    assert!(rig.driver.shared.tasks.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_panicking_rejection_follow_up_logs_and_releases_its_channel() {
    let rig = rig(vec![Step::Panic, Step::Reply("after")]).await;
    rig.driver.rejected(rejected_card("9001", CHANNEL)).await;
    rig.settle().await;
    assert_eq!(
        rig.effects(),
        [
            format!(
                "post silent ^9001 {CHANNEL}: {}",
                kanade().staging_lines().generic
            ),
            format!("edit 5001: {FAILURE_REPLY}"),
        ],
        "an idle staged placeholder becomes the failure line"
    );
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].guardrail["kind"], "rejection_followup");
    assert_eq!(rows[0].guardrail["delivery"]["cause"], "aborted");
    assert_eq!(
        rows[0].error.as_deref(),
        Some("failed: the question stopped unexpectedly")
    );
    assert!(
        !rig.driver.state().rejection_followups.contains(CHANNEL),
        "the aborted task released the channel"
    );

    rig.advance(31.0);
    rig.driver.rejected(rejected_card("9002", CHANNEL)).await;
    rig.settle().await;
    assert!(
        rig.effects().iter().any(|effect| effect.contains("^9002")),
        "the later eligible rejection starts after cooldown"
    );
    assert_eq!(rig.rows().len(), 2);
}

#[tokio::test]
async fn a_hard_shutdown_abort_of_a_rejection_follow_up_logs_and_joins_it() {
    let rig = rig_with(
        Fake::new(vec![Step::Forever]),
        DriverConfig {
            stop_grace: Duration::from_millis(50),
            cut_budget: Duration::from_millis(50),
            ..DriverConfig::default()
        },
        &MemoryScheduleStore::new(),
    )
    .await;
    let _stuck = rig.fake_discord().hold(Op::Edit);
    rig.driver.rejected(rejected_card("9001", CHANNEL)).await;
    rig.settle().await;
    rig.driver.stop().await;
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].guardrail["kind"], "rejection_followup");
    assert_eq!(rows[0].guardrail["delivery"]["cause"], "shutdown");
    assert_eq!(rows[0].guardrail["delivery"]["unknown"], json!([1]));
    assert_eq!(
        rows[0].error.as_deref(),
        Some("cancelled: serve shut down; incomplete: delivered 0 of 1 parts")
    );
    assert!(
        !rig.driver.state().rejection_followups.contains(CHANNEL),
        "the abort release ran before stop returned"
    );
    assert!(rig.driver.shared.tasks.lock().unwrap().is_empty());
}

mod delivery;
