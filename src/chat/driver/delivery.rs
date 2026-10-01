//! One question's delivery (`docs/v5/chat-orchestration.md`, "Delivery"):
//! a silent staging placeholder replying to the question, typing while the
//! answer runs, then the answer edited into the placeholder (part 1) and
//! posted as silent continuations. State lives in memory only; the
//! [`DeliveryRecord`] is shared with the question's `Held` guard so an abort
//! sees what was delivered.
//!
//! Effects run in this task, one at a time (the placeholder's withdrawal runs
//! beside the still-running answer). A started effect is awaited to its
//! outcome; deletion and shutdown are observed only between effects. Retries:
//! a create that was never sent is retried once and an ambiguous create is
//! never replayed; an edit or delete gets at most one identical retry.

use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::time::Instant;

use super::{Effect, Post, Surface};

/// Appended to the last confirmed part of an answer that stopped early.
pub const INCOMPLETE_MARKER: &str = " *(reply incomplete)*";
/// Typing shows for about 10 s per trigger.
pub const TYPING_EVERY: Duration = Duration::from_secs(8);
/// Discord's message limit in UTF-16 units.
const DISCORD_UNITS: usize = 2000;

/// A running question's deletion: the flag the answerer reads and the
/// wake-up the delivery waits on. Setting it issues no Discord effect.
#[derive(Debug, Default)]
pub struct Deletion {
    pub flag: Arc<AtomicBool>,
    notify: Notify,
}

impl Deletion {
    pub fn set(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_one();
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    async fn wait(&self) {
        while !self.is_set() {
            self.notify.notified().await;
        }
    }
}

/// The staging placeholder, as far as the delivery knows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Placeholder {
    #[default]
    None,
    /// Posted and still showing the staging line.
    Live(String),
    /// Edited into part 1 or the failure line.
    Edited,
    Deleted,
    CreateRejected,
    CreateAmbiguous,
    /// Its withdrawal failed; it may still show the staging line.
    Orphaned,
}

impl Placeholder {
    fn label(&self) -> &'static str {
        match self {
            Self::None => "none",
            // Still live at the end: nothing replaced it.
            Self::Live(_) | Self::Orphaned => "orphaned",
            Self::Edited => "edited",
            Self::Deleted => "deleted",
            Self::CreateRejected => "create_rejected",
            Self::CreateAmbiguous => "create_ambiguous",
        }
    }
}

/// Why an answer was not (fully) delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Rejected,
    Ambiguous,
    Deleted,
    Shutdown,
    Aborted,
}

impl Cause {
    fn label(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Ambiguous => "ambiguous",
            Self::Deleted => "deleted",
            Self::Shutdown => "shutdown",
            Self::Aborted => "aborted",
        }
    }
}

/// An effect whose transport future has been polled but has not returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InFlight {
    Staging,
    /// Part `k` (1-based): its create or the edit into the placeholder.
    Part(usize),
    Withdraw,
    Marker,
}

/// A part counted as delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landed {
    pub index: usize,
    pub id: String,
    pub text: String,
    /// `false`: an ambiguous edit treated as landed (D2).
    pub confirmed: bool,
}

/// What one question's delivery did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeliveryRecord {
    pub placeholder: Placeholder,
    /// Answer parts (0 until the answer is split).
    pub parts: usize,
    pub landed: Vec<Landed>,
    /// Parts whose landing is unknown (1-based).
    pub unknown: Vec<usize>,
    pub incomplete: bool,
    pub cause: Option<Cause>,
    pub in_flight: Option<InFlight>,
    /// An answer-carrying effect has started.
    pub answer_started: bool,
}

impl DeliveryRecord {
    /// The anchor id: part 1's (the placeholder's on the edit path).
    pub fn first_id(&self) -> Option<String> {
        self.landed.first().map(|landed| landed.id.clone())
    }

    fn stop(&mut self, cause: Cause) {
        self.cause.get_or_insert(cause);
        if self.parts > 0 && self.landed.len() < self.parts {
            self.incomplete = true;
        }
    }

    /// The final hard abort dropped whatever was in flight: it is ambiguous.
    pub fn aborted(&mut self, cause: Cause) {
        match self.in_flight.take() {
            Some(InFlight::Staging) => self.placeholder = Placeholder::CreateAmbiguous,
            Some(InFlight::Part(index)) => self.unknown.push(index),
            Some(InFlight::Withdraw) => self.placeholder = Placeholder::Orphaned,
            Some(InFlight::Marker) | None => {}
        }
        if self.parts == 0 || self.landed.len() < self.parts {
            self.cause.get_or_insert(cause);
        }
        if self.parts > 0 && self.landed.len() < self.parts {
            self.incomplete = true;
        }
    }

    /// The chat row's `guardrail.delivery`: `placeholder`, `parts` and
    /// `delivered` always; `unknown`, `incomplete` and `cause` only when set.
    pub fn guardrail(&self) -> Value {
        let mut out = json!({
            "placeholder": self.placeholder.label(),
            "parts": self.parts,
            "delivered": self.landed.len(),
        });
        let map = out.as_object_mut().expect("object");
        if !self.unknown.is_empty() {
            map.insert("unknown".into(), json!(self.unknown));
        }
        if self.incomplete {
            map.insert("incomplete".into(), json!(true));
        }
        if let Some(cause) = self.cause {
            map.insert("cause".into(), json!(cause.label()));
        }
        out
    }

    /// `incomplete: delivered k of n parts` for an answer that stopped early.
    pub fn incomplete_error(&self) -> Option<String> {
        self.incomplete.then(|| {
            format!(
                "incomplete: delivered {} of {} parts",
                self.landed.len(),
                self.parts
            )
        })
    }
}

/// Append `extra` to a row error.
pub fn append_error(error: &mut Option<String>, extra: String) {
    *error = Some(match error.take() {
        Some(existing) => format!("{existing}; {extra}"),
        None => extra,
    });
}

pub fn lock(record: &Mutex<DeliveryRecord>) -> MutexGuard<'_, DeliveryRecord> {
    record.lock().unwrap_or_else(PoisonError::into_inner)
}

type Fx<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A slot's future, or never.
async fn slot<T>(slot: &mut Option<Fx<'_, T>>) -> T {
    match slot {
        Some(future) => future.await,
        None => std::future::pending().await,
    }
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// How the edit of part 1 into the placeholder ended.
enum FirstEdit {
    Landed,
    /// The placeholder is gone: post part 1 as a new reply.
    Gone,
    /// Refused: post part 1 as a new reply, then delete the placeholder.
    Refused,
}

pub struct Delivery<'a, S> {
    surface: &'a S,
    channel: &'a str,
    question: &'a str,
    record: Arc<Mutex<DeliveryRecord>>,
    deletion: &'a Deletion,
}

impl<'a, S: Surface> Delivery<'a, S> {
    pub fn new(
        surface: &'a S,
        channel: &'a str,
        question: &'a str,
        record: Arc<Mutex<DeliveryRecord>>,
        deletion: &'a Deletion,
    ) -> Self {
        Self {
            surface,
            channel,
            question,
            record,
            deletion,
        }
    }

    fn record(&self) -> MutexGuard<'_, DeliveryRecord> {
        lock(&self.record)
    }

    /// Mark `what` in flight from its first poll until it returns.
    fn tracked<T: Send + 'a>(
        &self,
        what: InFlight,
        effect: impl Future<Output = Effect<T>> + Send + 'a,
    ) -> Fx<'a, Effect<T>> {
        let record = Arc::clone(&self.record);
        Box::pin(async move {
            {
                let mut record = lock(&record);
                record.in_flight = Some(what);
                if matches!(what, InFlight::Part(_)) {
                    record.answer_started = true;
                }
            }
            let outcome = effect.await;
            lock(&record).in_flight = None;
            outcome
        })
    }

    fn post(
        &self,
        what: InFlight,
        text: String,
        reply_to: bool,
        silent: bool,
    ) -> Fx<'a, Effect<String>> {
        let (surface, channel, question) = (self.surface, self.channel, self.question);
        self.tracked(what, async move {
            let post = Post {
                text: &text,
                reply_to: reply_to.then_some(question),
                silent,
            };
            surface.post(channel, post).await
        })
    }

    fn edit(&self, what: InFlight, id: String, text: String) -> Fx<'a, Effect<()>> {
        let (surface, channel) = (self.surface, self.channel);
        self.tracked(what, async move { surface.edit(channel, &id, &text).await })
    }

    fn delete(&self, id: String) -> Fx<'a, Effect<()>> {
        let (surface, channel) = (self.surface, self.channel);
        self.tracked(InFlight::Withdraw, async move {
            surface.delete(channel, &id).await
        })
    }

    fn typing(&self) -> Fx<'a, ()> {
        let (surface, channel) = (self.surface, self.channel);
        Box::pin(async move { surface.typing(channel).await })
    }

    /// Stage, type and withdraw around the running answer until it returns
    /// (`Some`) or shutdown cuts it (`None`, the answer dropped), and every
    /// started effect has returned. The answer is never dropped for a
    /// deletion.
    pub async fn until_answered<T>(
        &self,
        staging: String,
        answer: impl Future<Output = T>,
        cut: impl Future<Output = ()>,
    ) -> Option<T> {
        let mut answer = pin!(answer);
        let mut cut = pin!(cut);
        let mut result: Option<Option<T>> = None;
        let mut deleted = self.deletion.is_set();
        let mut create: Option<Fx<'a, Effect<String>>> = None;
        let mut create_retried = false;
        let mut withdraw: Option<Fx<'a, Effect<()>>> = None;
        let mut withdraw_retried = false;
        let mut typing: Option<Fx<'a, ()>> = None;
        let mut next_tick: Option<Instant> = None;
        if !deleted {
            create = Some(self.post(InFlight::Staging, staging.clone(), true, true));
        }
        loop {
            if result.is_some() && create.is_none() && withdraw.is_none() {
                break;
            }
            let ticking = result.is_none() && !deleted && typing.is_none() && next_tick.is_some();
            let tick_at = next_tick.unwrap_or_else(Instant::now);
            tokio::select! {
                biased;
                // Deletion and the cut first, so an effect returning in the
                // same poll already sees them.
                () = self.deletion.wait(), if !deleted => {
                    deleted = true;
                    let live = self.live();
                    if let (Some(id), None) = (live, &withdraw) {
                        withdraw = Some(self.delete(id));
                    }
                }
                () = &mut cut, if result.is_none() => result = Some(None),
                outcome = slot(&mut create) => {
                    create = None;
                    match outcome {
                        Effect::Done(id) => {
                            self.record().placeholder = Placeholder::Live(id.clone());
                            next_tick = Some(Instant::now());
                            if deleted {
                                withdraw = Some(self.delete(id));
                            }
                        }
                        // Never re-posted for a question deleted or cut meanwhile.
                        Effect::NotSent if !create_retried && !self.deletion.is_set() && !matches!(result, Some(None)) => {
                            create_retried = true;
                            create = Some(self.post(InFlight::Staging, staging.clone(), true, true));
                        }
                        Effect::Ambiguous(_) => {
                            self.record().placeholder = Placeholder::CreateAmbiguous;
                            next_tick = Some(Instant::now());
                        }
                        _ => {
                            self.record().placeholder = Placeholder::CreateRejected;
                            next_tick = Some(Instant::now());
                        }
                    }
                }
                outcome = slot(&mut withdraw) => {
                    withdraw = None;
                    if let Some(id) = self.withdrawn(outcome, &mut withdraw_retried) {
                        withdraw = Some(self.delete(id));
                    }
                }
                () = slot(&mut typing) => typing = None,
                output = &mut answer, if result.is_none() => result = Some(Some(output)),
                () = tokio::time::sleep_until(tick_at), if ticking => {
                    typing = Some(self.typing());
                    next_tick = Some(Instant::now() + TYPING_EVERY);
                }
            }
        }
        // A typing trigger still in flight is dropped: it carries nothing.
        result.flatten()
    }

    fn live(&self) -> Option<String> {
        match &self.record().placeholder {
            Placeholder::Live(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// Settle one withdrawal attempt; `Some(id)` to retry it once.
    fn withdrawn(&self, outcome: Effect<()>, retried: &mut bool) -> Option<String> {
        let mut record = self.record();
        let Placeholder::Live(id) = record.placeholder.clone() else {
            return None;
        };
        match outcome {
            Effect::Done(()) | Effect::UnknownMessage => record.placeholder = Placeholder::Deleted,
            Effect::NotSent | Effect::Ambiguous(_) if !*retried => {
                *retried = true;
                return Some(id);
            }
            _ => record.placeholder = Placeholder::Orphaned,
        }
        None
    }

    /// Delete a placeholder that never carried an answer.
    pub async fn withdraw(&self) {
        let mut retried = false;
        while let Some(id) = self.live() {
            let outcome = self.delete(id).await;
            if self.withdrawn(outcome, &mut retried).is_none() {
                return;
            }
        }
    }

    /// The question ended before its first answer effect (deleted, or cut
    /// without a placeholder).
    pub async fn ended_early(&self, cause: Cause) {
        self.withdraw().await;
        self.record().stop(cause);
    }

    /// Deliver `parts` in order: part 1 edited into a live placeholder (or
    /// posted as a reply), continuations posted silently, each only after
    /// the previous one landed. A deletion seen between parts, or a part
    /// that did not land, ends it with the incomplete marker.
    pub async fn deliver(&self, parts: Vec<String>) {
        self.record().parts = parts.len();
        let Some(first) = parts.first() else {
            return;
        };
        let landed_first = match self.live() {
            Some(id) => match self.edit_first(&id, first).await {
                FirstEdit::Landed => Ok(()),
                edit => {
                    // Nothing of the answer landed yet: a deleted question
                    // only loses its placeholder (withdrawn, not incomplete,
                    // and no marker).
                    if self.deletion.is_set() {
                        self.withdraw().await;
                        self.record().cause.get_or_insert(Cause::Deleted);
                        return;
                    }
                    let posted = self.post_part(1, first).await;
                    if matches!(edit, FirstEdit::Refused) {
                        self.withdraw().await;
                    }
                    posted
                }
            },
            None => self.post_part(1, first).await,
        };
        if let Err(cause) = landed_first {
            return self.end(cause).await;
        }
        for (index, part) in parts.iter().enumerate().skip(1) {
            if self.deletion.is_set() {
                return self.end(Cause::Deleted).await;
            }
            if let Err(cause) = self.post_part(index + 1, part).await {
                return self.end(cause).await;
            }
        }
    }

    async fn edit_first(&self, id: &str, text: &str) -> FirstEdit {
        let attempt = || self.edit(InFlight::Part(1), id.to_owned(), text.to_owned());
        let first = attempt().await;
        let after_ambiguous = matches!(first, Effect::Ambiguous(_));
        let outcome = match first {
            Effect::NotSent | Effect::Ambiguous(_) => attempt().await,
            other => other,
        };
        let landed = |confirmed: bool| {
            let mut record = self.record();
            record.placeholder = Placeholder::Edited;
            if !confirmed {
                record.unknown.push(1);
            }
            record.landed.push(Landed {
                index: 1,
                id: id.to_owned(),
                text: text.to_owned(),
                confirmed,
            });
            FirstEdit::Landed
        };
        match outcome {
            Effect::Done(()) => landed(true),
            Effect::UnknownMessage => {
                self.record().placeholder = Placeholder::Deleted;
                FirstEdit::Gone
            }
            // D2: still unknown, or refused after an edit that may have
            // landed (never deleted or rewritten then): treat as landed.
            Effect::Ambiguous(_) => landed(false),
            _ if after_ambiguous => landed(false),
            _ => FirstEdit::Refused,
        }
    }

    /// Post part `k` (1-based): part 1 replies to the question (audible, as
    /// before), continuations are silent and reply to nothing.
    async fn post_part(&self, index: usize, text: &str) -> Result<(), Cause> {
        let attempt = || {
            self.post(
                InFlight::Part(index),
                text.to_owned(),
                index == 1,
                index > 1,
            )
        };
        let outcome = match attempt().await {
            Effect::NotSent => attempt().await,
            other => other,
        };
        match outcome {
            Effect::Done(id) => {
                self.record().landed.push(Landed {
                    index,
                    id,
                    text: text.to_owned(),
                    confirmed: true,
                });
                Ok(())
            }
            Effect::Ambiguous(_) => {
                self.record().unknown.push(index);
                Err(Cause::Ambiguous)
            }
            _ => Err(Cause::Rejected),
        }
    }

    /// Ending: mark the answer incomplete on its last confirmed part, or in
    /// one silent new message when it would not fit or none was confirmed.
    async fn end(&self, cause: Cause) {
        let last = {
            let mut record = self.record();
            record.stop(cause);
            record
                .landed
                .iter()
                .rev()
                .find(|landed| landed.confirmed)
                .map(|landed| {
                    (
                        landed.id.clone(),
                        format!("{}{INCOMPLETE_MARKER}", landed.text),
                    )
                })
        };
        match last.filter(|(_, text)| units(text) <= DISCORD_UNITS) {
            Some((id, text)) => {
                let attempt = || self.edit(InFlight::Marker, id.clone(), text.clone());
                if matches!(attempt().await, Effect::NotSent | Effect::Ambiguous(_)) {
                    attempt().await;
                }
            }
            None => {
                let reply_to = self.record().landed.is_empty();
                let marker = INCOMPLETE_MARKER.trim_start().to_owned();
                let attempt = || self.post(InFlight::Marker, marker.clone(), reply_to, true);
                if attempt().await == Effect::NotSent {
                    attempt().await;
                }
            }
        }
    }
}
