//! A stored completion prompt (`run_prompts`, migration 0035) and its store
//! port. One row per ask of one run: "Not yet" closes an ask and opens the
//! next, a moved run's ask closes and a new one is planned from its new end.
//! At most one ask per run is open at a time.

use std::pin::Pin;

use chrono::{DateTime, Utc};

use crate::domain::scheduler::StoreError;

/// A boxed store answer, so the port can sit behind `dyn` (Discord presses)
/// as well as a generic (the delivery tick).
pub type PromptFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, StoreError>> + Send + 'a>>;

/// How an ask was closed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptOutcome {
    /// A party member or admin pressed Done.
    Done,
    /// … pressed Didn't happen: the run is cancelled.
    DidntHappen,
    /// … pressed Not yet: the next ask is due later.
    NotYet,
    /// Unanswered at the cutoff: Kanade marked the run done.
    AutoDone,
    /// The run moved after this ask was planned; a new one replaces it.
    Moved,
    /// The run became done, cancelled or own time another way (or is gone).
    Closed,
}

impl PromptOutcome {
    pub const ALL: &[Self] = &[
        Self::Done,
        Self::DidntHappen,
        Self::NotYet,
        Self::AutoDone,
        Self::Moved,
        Self::Closed,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::DidntHappen => "didnt_happen",
            Self::NotYet => "not_yet",
            Self::AutoDone => "auto_done",
            Self::Moved => "moved",
            Self::Closed => "closed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|outcome| outcome.as_str() == text)
    }

    /// Pressed by someone, who is recorded.
    pub fn pressed(self) -> bool {
        matches!(self, Self::Done | Self::DidntHappen | Self::NotYet)
    }
}

/// One ask of one run's completion prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunPrompt {
    pub run_id: String,
    /// 0 for the first ask, one more for each later one.
    pub ask: u32,
    /// The run end this ask was planned from; a different end means it moved.
    pub ends_at: DateTime<Utc>,
    /// When this ask posts.
    pub due_at: DateTime<Utc>,
    /// When the run is marked done unanswered.
    pub cutoff_at: DateTime<Utc>,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    /// `None` while open.
    pub outcome: Option<PromptOutcome>,
    /// The presser's user id, for a pressed outcome only.
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    /// Its posted message now shows the outcome (or is gone).
    pub message_settled: bool,
}

impl RunPrompt {
    /// A new open ask.
    pub fn open(
        run_id: String,
        ask: u32,
        ends_at: DateTime<Utc>,
        due_at: DateTime<Utc>,
        cutoff_at: DateTime<Utc>,
    ) -> Self {
        Self {
            run_id,
            ask,
            ends_at,
            due_at,
            cutoff_at,
            channel_id: None,
            message_id: None,
            outcome: None,
            decided_by: None,
            decided_at: None,
            message_settled: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.outcome.is_none()
    }

    /// `<run id>:<ask>`, as its journal source key and buttons name it.
    pub fn key(&self) -> String {
        format!("{}:{}", self.run_id, self.ask)
    }

    /// The shape both stores enforce (SQLite by CHECK as well).
    ///
    /// # Errors
    /// [`StoreError::Constraint`] naming the first bad field.
    pub fn check(&self) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("run prompt {what}")));
        let bounded = |text: &str, max: usize| !text.is_empty() && text.len() <= max;
        if !bounded(&self.run_id, 64)
            || !self
                .decided_by
                .as_deref()
                .is_none_or(|user| bounded(user, 32))
        {
            return refuse("ids are bounded");
        }
        if self.cutoff_at <= self.due_at {
            return refuse("is cut off after it is due");
        }
        if self.outcome.is_none() != self.decided_at.is_none() {
            return refuse("is decided exactly when closed");
        }
        if self.outcome.is_some_and(PromptOutcome::pressed) != self.decided_by.is_some() {
            return refuse("names its presser exactly when pressed");
        }
        Ok(())
    }

    /// [`Self::check`] for a new ask, which must be open and unposted.
    ///
    /// # Errors
    /// [`StoreError::Constraint`].
    pub fn check_new(&self) -> Result<(), StoreError> {
        if !self.is_open() || self.message_id.is_some() || self.message_settled {
            return Err(StoreError::Constraint(
                "a new run prompt is open and unposted".into(),
            ));
        }
        self.check()
    }
}

/// How an open ask closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PromptClose {
    pub outcome: PromptOutcome,
    /// The presser's user id for a pressed outcome, else `None`.
    pub decided_by: Option<String>,
    pub at: DateTime<Utc>,
    /// The ask that replaces it ("Not yet", a move), stored in the same write.
    pub next: Option<RunPrompt>,
}

impl PromptClose {
    /// The shape both stores refuse before writing anything.
    ///
    /// # Errors
    /// [`StoreError::Constraint`].
    pub fn check(&self, run_id: &str) -> Result<(), StoreError> {
        let refuse = |what: &str| Err(StoreError::Constraint(format!("run prompt close {what}")));
        if self.outcome.pressed() != self.decided_by.is_some() {
            return refuse("names its presser exactly when pressed");
        }
        if let Some(next) = &self.next {
            if next.run_id != run_id {
                return refuse("opens its next ask on the same run");
            }
            next.check_new()?;
        }
        Ok(())
    }
}

pub trait RunPromptStore: Send + Sync {
    /// Store a new open ask. An existing `(run, ask)` or another open ask of
    /// the run is [`StoreError::Constraint`] and nothing is written.
    fn create_run_prompt(&self, prompt: RunPrompt) -> PromptFuture<'_, ()>;

    fn run_prompt(&self, run_id: String, ask: u32) -> PromptFuture<'_, Option<RunPrompt>>;

    /// The run's highest ask, if any.
    fn latest_run_prompt(&self, run_id: String) -> PromptFuture<'_, Option<RunPrompt>>;

    /// Every open ask, earliest due first.
    fn open_run_prompts(&self) -> PromptFuture<'_, Vec<RunPrompt>>;

    /// Close `(run, ask)` if it is still open, then store `close.next` in
    /// the same write; `false` (nothing written) when it was already closed.
    fn close_run_prompt(
        &self,
        run_id: String,
        ask: u32,
        close: PromptClose,
    ) -> PromptFuture<'_, bool>;

    /// Record where the ask was posted.
    fn set_run_prompt_message(
        &self,
        run_id: String,
        ask: u32,
        channel_id: String,
        message_id: String,
    ) -> PromptFuture<'_, ()>;

    /// Closed asks whose posted message still shows the buttons, oldest
    /// decision first.
    fn unsettled_run_prompts(&self) -> PromptFuture<'_, Vec<RunPrompt>>;

    /// Its message now shows the outcome (or is gone).
    fn settle_run_prompt_message(&self, run_id: String, ask: u32) -> PromptFuture<'_, ()>;
}
