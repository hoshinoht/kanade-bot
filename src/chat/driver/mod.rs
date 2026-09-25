//! Serve's chat loop around [`ChatPilot`], with no Discord types: the gate,
//! the per-channel queue (position reactions, refunds on shed, delete and
//! expiry), the clean-retry reservation taken when a question is dequeued,
//! the answer, `conclude` with the reservation it holds, the reply and the
//! chat-log row. The model side is an [`Answerer`], Discord a [`Surface`]
//! (`docs/v5/chat-orchestration.md`, "Serve composition").

mod ports;
mod run;
mod view;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;

pub use ports::{Answerer, Asked, Job, Prepared, Setup, Surface};
pub use view::{ChatHandle, ChatView};

use crate::chat::gate::{CHANNEL_BUSY_REACTION, ChatDecision, RATE_LIMITED_REACTION, Summons};
use crate::chat::pilot::{Admission, ChatPilot, GuardLimits, LimitsView, LogFacts, TrafficLimits};
use crate::chat::tools::ToolContext;
use crate::domain::model_log::{AllowanceOverride, ModelLogStore};
use crate::domain::scheduler::StoreError;
use crate::infrastructure::llm::governor::MAX_TOOL_ROUNDS;

/// v4 `CHAT_PILOT_TIMEOUT`: one question, queueing for a permit included.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// v4 `CHAT_PILOT_HISTORY_TTL_S`.
pub const DEFAULT_HISTORY_TTL_S: f64 = 2700.0;
/// v4 `MODEL_CONTEXT_TOKENS`.
pub const DEFAULT_CONTEXT_TOKENS: usize = 8192;

/// Monotonic seconds.
pub type Monotonic = Arc<dyn Fn() -> f64 + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriverConfig {
    pub traffic: TrafficLimits,
    pub guard: GuardLimits,
    pub history_ttl_s: f64,
    pub timeout: Duration,
    pub tool_rounds: u8,
    pub model_context_tokens: usize,
    pub max_output_tokens: u32,
    /// How long shutdown lets running answers finish before cutting them.
    pub stop_grace: Duration,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            traffic: TrafficLimits::default(),
            guard: GuardLimits::default(),
            history_ttl_s: DEFAULT_HISTORY_TTL_S,
            timeout: DEFAULT_TIMEOUT,
            tool_rounds: crate::infrastructure::llm::governor::DEFAULT_TOOL_ROUNDS,
            model_context_tokens: DEFAULT_CONTEXT_TOKENS,
            max_output_tokens: crate::chat::context::COMPLETION_RESERVE_TOKENS as u32,
            stop_grace: Duration::from_secs(5),
        }
    }
}

impl DriverConfig {
    /// The question timeout must stay below the clean-retry window, or the
    /// guard's `prune` could drop a live reservation.
    pub fn validate(&self) -> Result<(), String> {
        let seconds = self.timeout.as_secs_f64();
        if seconds <= 0.0 || seconds >= self.guard.per_member_s {
            return Err(format!(
                "the chat question timeout ({seconds} s) must be above 0 and below the clean-retry window ({} s)",
                self.guard.per_member_s
            ));
        }
        if !(1..=MAX_TOOL_ROUNDS).contains(&self.tool_rounds) {
            return Err(format!("chat tool rounds must be 1..={MAX_TOOL_ROUNDS}"));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum DriverError {
    Config(String),
    /// Withheld ids could not be reloaded; nothing may be admitted.
    Store(StoreError),
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(reason) => f.write_str(reason),
            Self::Store(error) => write!(f, "chat log unreadable: {error}"),
        }
    }
}

impl std::error::Error for DriverError {}

/// Keycap for a 1-based queue position (🔟 from ten on).
pub fn position_reaction(position: usize) -> &'static str {
    const KEYCAPS: [&str; 9] = [
        "1\u{fe0f}\u{20e3}",
        "2\u{fe0f}\u{20e3}",
        "3\u{fe0f}\u{20e3}",
        "4\u{fe0f}\u{20e3}",
        "5\u{fe0f}\u{20e3}",
        "6\u{fe0f}\u{20e3}",
        "7\u{fe0f}\u{20e3}",
        "8\u{fe0f}\u{20e3}",
        "9\u{fe0f}\u{20e3}",
    ];
    KEYCAPS
        .get(position.wrapping_sub(1))
        .copied()
        .unwrap_or("🔟")
}

/// A question accepted by the gate, waiting or about to run.
#[derive(Clone)]
struct Queued {
    asked: Asked,
    spent_at: Option<f64>,
    /// Its queue position reaction, if it waited.
    position: Option<usize>,
}

struct State {
    pilot: ChatPilot,
    overrides: Vec<AllowanceOverride>,
    waiting: HashMap<String, Queued>,
    /// Running questions by message id → deleted.
    running: HashMap<String, Arc<AtomicBool>>,
    persona_key: Option<String>,
    closed: bool,
}

struct Shared<A, S> {
    answerer: A,
    surface: S,
    config: DriverConfig,
    monotonic: Monotonic,
    state: Mutex<State>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Shutdown past its grace: running answers are dropped.
    cut: watch::Sender<bool>,
}

/// The running chat pilot; cheap to clone.
pub struct ChatDriver<A, S> {
    shared: Arc<Shared<A, S>>,
}

impl<A, S> Clone for ChatDriver<A, S> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

fn new_row_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

impl<A: Answerer, S: Surface> ChatDriver<A, S> {
    /// Validate the config and reload the withheld ids from the chat log
    /// before the driver exists, so no question is admitted first.
    pub async fn start<L: ModelLogStore + Sync>(
        config: DriverConfig,
        answerer: A,
        surface: S,
        log: &L,
        monotonic: Monotonic,
    ) -> Result<Self, DriverError> {
        config.validate().map_err(DriverError::Config)?;
        let mut pilot = ChatPilot::new(config.history_ttl_s, config.traffic, config.guard);
        pilot
            .reload_withheld(log)
            .await
            .map_err(DriverError::Store)?;
        let overrides = log
            .allowance_overrides()
            .await
            .map_err(DriverError::Store)?;
        let (cut, _) = watch::channel(false);
        Ok(Self {
            shared: Arc::new(Shared {
                answerer,
                surface,
                config,
                monotonic,
                state: Mutex::new(State {
                    pilot,
                    overrides,
                    waiting: HashMap::new(),
                    running: HashMap::new(),
                    persona_key: None,
                    closed: false,
                }),
                tasks: Mutex::new(Vec::new()),
                cut,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> f64 {
        (self.shared.monotonic)()
    }

    fn spawn(&self, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = self
            .shared
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tasks.retain(|task| !task.is_finished());
        tasks.push(tokio::spawn(task));
    }

    /// Per-member allowance overrides (the store's rows), replacing the old.
    pub fn set_overrides(&self, rows: Vec<AllowanceOverride>) {
        self.state().overrides = rows;
    }

    /// Gate one message and admit it. `true` when chat took it (answered,
    /// queued, shed or rate-limited): it is then not extraction input.
    pub fn offer(&self, asked: Asked) -> bool {
        let setup = self.shared.answerer.setup();
        let now = self.now();
        let mut guard = self.state();
        let state = &mut *guard;
        if state.closed {
            return false;
        }
        let pilot = &mut state.pilot;
        pilot
            .allowance
            .apply(setup.member_rate, setup.pool_rate, &state.overrides);
        let summons = Summons {
            bot_user_id: asked.bot_user_id.as_deref(),
            self_role_id: None,
            replied_author_id: asked.replied_author_id.as_deref(),
            enabled: setup.enabled && setup.ready,
            is_admin: asked.is_admin,
        };
        let decision = crate::chat::gate::decide(
            &asked.gate,
            &setup.pilot,
            self.shared.answerer.channels(),
            summons,
            pilot.allowance.budgets(now),
        );
        if !decision.act {
            if decision.busy {
                self.limited(pilot, &setup, &asked, &decision, now);
                return true;
            }
            return false;
        }
        let spent_at = (!asked.is_admin).then_some(now);
        let message_id = asked.message.id.clone();
        let admission = pilot.traffic.admit(
            &asked.origin_id,
            &message_id,
            &asked.message.author_id,
            (now, spent_at),
        );
        match admission {
            Admission::Answer => {
                let cancelled = Arc::new(AtomicBool::new(false));
                state.running.insert(message_id, Arc::clone(&cancelled));
                drop(guard);
                let job = Queued {
                    asked,
                    spent_at,
                    position: None,
                };
                self.spawn(self.clone().worker(job, cancelled));
            }
            Admission::Queued { position } => {
                let (channel, emoji) = (asked.channel_id.clone(), position_reaction(position));
                state.waiting.insert(
                    message_id.clone(),
                    Queued {
                        asked,
                        spent_at,
                        position: Some(position),
                    },
                );
                drop(guard);
                let driver = self.clone();
                self.spawn(async move {
                    driver
                        .shared
                        .surface
                        .react(&channel, &message_id, emoji)
                        .await;
                });
            }
            Admission::Busy => {
                if let Some(stamp) = spent_at {
                    pilot.allowance.refund(&asked.message.author_id, stamp);
                }
                drop(guard);
                let driver = self.clone();
                self.spawn(async move {
                    let surface = &driver.shared.surface;
                    surface
                        .react(&asked.channel_id, &message_id, CHANNEL_BUSY_REACTION)
                        .await;
                });
            }
        }
        true
    }

    /// A spent budget: the reaction, the once-per-episode reply and the
    /// `rate_limited` row.
    fn limited(
        &self,
        pilot: &mut ChatPilot,
        setup: &Setup,
        asked: &Asked,
        decision: &ChatDecision,
        now: f64,
    ) {
        let ctx = ToolContext::new(
            asked.message.author_id.clone(),
            asked.origin_id.clone(),
            asked.message.id.clone(),
            setup.now,
        );
        let facts = LogFacts {
            id: new_row_id(),
            at: setup.now,
            model: &setup.model,
            reasoning: None,
            latency_ms: 0,
        };
        let (reply, row) = pilot.limited(&ctx, &asked.message.content, decision, facts, now);
        let (channel, message) = (asked.channel_id.clone(), asked.message.id.clone());
        let driver = self.clone();
        self.spawn(async move {
            let shared = &driver.shared;
            shared
                .surface
                .react(&channel, &message, RATE_LIMITED_REACTION)
                .await;
            if let Some(text) = reply {
                let _ = shared.surface.reply(&channel, &message, &text).await;
            }
            shared.answerer.record(row).await;
        });
    }

    /// Deleted messages: a waiting question leaves the queue with a refund;
    /// a running one finishes (a staged proposal is never cut from its
    /// supersede) but posts nothing more.
    pub fn deleted(&self, message_ids: &[String]) {
        let mut guard = self.state();
        let state = &mut *guard;
        for id in message_ids {
            if let Some(waiting) = state.pilot.traffic.cancel(id) {
                state.waiting.remove(id);
                if let Some(stamp) = waiting.spent_at {
                    state.pilot.allowance.refund(&waiting.member_id, stamp);
                }
            } else if let Some(cancelled) = state.running.get(id) {
                cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Stop admitting, refund every waiting question, give running answers
    /// the grace to finish, then cut the rest (each still concludes).
    pub async fn stop(&self) {
        let dropped: Vec<Queued> = {
            let mut guard = self.state();
            let state = &mut *guard;
            state.closed = true;
            let ids: Vec<String> = state.waiting.keys().cloned().collect();
            ids.iter()
                .filter_map(|id| {
                    let waiting = state.pilot.traffic.cancel(id)?;
                    if let Some(stamp) = waiting.spent_at {
                        state.pilot.allowance.refund(&waiting.member_id, stamp);
                    }
                    state.waiting.remove(id)
                })
                .collect()
        };
        let deadline = tokio::time::Instant::now() + self.shared.config.stop_grace;
        for queued in &dropped {
            if let Some(position) = queued.position {
                let asked = &queued.asked;
                let unreact = self.shared.surface.unreact(
                    &asked.channel_id,
                    &asked.message.id,
                    position_reaction(position),
                );
                let _ = tokio::time::timeout_at(deadline, unreact).await;
            }
        }
        let tasks: Vec<JoinHandle<()>> = std::mem::take(
            &mut *self
                .shared
                .tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let mut left = Vec::new();
        for mut task in tasks {
            if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
                left.push(task);
            }
        }
        self.shared.cut.send_replace(true);
        for task in left {
            let _ = task.await;
        }
    }

    /// The Limits page's view.
    pub fn limits(&self) -> LimitsView {
        let now = self.now();
        self.state().pilot.limits(now)
    }

    /// `disabled`, `idle`, `busy` or `degraded` (enabled but unable to
    /// answer, or clean retries suspended by the storm guard).
    pub fn status(&self) -> &'static str {
        let setup = self.shared.answerer.setup();
        if !setup.enabled {
            return "disabled";
        }
        if !setup.ready || !setup.pilot.configured() {
            return "degraded";
        }
        let now = self.now();
        let mut state = self.state();
        if state.pilot.guard.view(now).suspended_until.is_some() {
            "degraded"
        } else if state.pilot.traffic.view().answering.is_empty() {
            "idle"
        } else {
            "busy"
        }
    }
}

impl<A: Answerer, S: Surface> ChatView for ChatDriver<A, S> {
    fn limits(&self) -> LimitsView {
        ChatDriver::limits(self)
    }

    fn status(&self) -> &'static str {
        ChatDriver::status(self)
    }
}
