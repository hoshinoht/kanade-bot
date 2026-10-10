//! Run completion on the tick (user decisions 2026-10-09/10). Half an hour
//! after a run ends Kanade posts a prompt in its channel (Done / Didn't
//! happen / Not yet, pinging nobody); a press is answered by
//! `bot::commands::runs` and recorded on the ask. Each tick:
//!
//! * [`Delivery::complete_runs`] (where v4 retired past runs) marks every
//!   live run past its cutoff done as Kanade, recorded with an `auto-done`
//!   request id, and closes its open ask as automatic;
//! * [`Delivery::run_prompts_in`] closes asks whose run ended another way or
//!   is gone, re-plans asks of a moved run, opens a run's first ask once it
//!   is due, posts due asks and edits closed asks' posts into their outcome
//!   (buttons removed).
//!
//! A post is a target-less journalled effect claimed by the durable source
//! key `run-prompt:<run>:<ask>`, so a restart, a replayed tick or an
//! ambiguous send never posts an ask twice. Posts and edits are Components
//! V2 and mention nobody; status changes they record ask for no notice.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::ButtonStyle;

use super::alerts::{AdminAlert, AlertSink};
use super::cards::redesign::{
    AT_RISK_RED, ButtonId, INK_BLUE, SETTLED_GREEN, WAITING_AMBER, action_row, button, container,
    subtext, text,
};
use super::executor::SendOutcome;
use super::tick::{Delivery, DeliveryError, settle};
use crate::bot::commands::text::{format_bosses, local_day, local_time, status_label};
use crate::bot::ids::{id_text, parse_id};
use crate::bot::mentions;
use crate::bot::transport::{
    DiscordTransport, MessageEdit, Outcome, OutgoingMessage, RejectionKind,
};
use crate::domain::completion::{
    ASK_AGAIN_AFTER, CompletionPlan, PromptClose, PromptOutcome, RunPrompt, RunPromptStore,
    auto_request_id, plan, pressed_request_id, run_minutes,
};
use crate::domain::drafts::ProposalStore;
use crate::domain::history::{Actor, Checkpoints, Origin, Surface};
use crate::domain::ids::short_id;
use crate::domain::notify::{
    ChannelChoice, DeliveryJournal, EffectKind, IntentContent, Lease, NoticeOutbox,
    NotificationIntent, choose_channel,
};
use crate::domain::schedule::{Run, RunStatus, SettleRun};
use crate::domain::scheduler::{IdSource, ScheduleStore, SchedulerError, Scope};
use crate::domain::time::DateOutOfRange;

/// The post's journal effect kind.
pub const RUN_PROMPT_EFFECT: &str = "notice.run.prompt";

pub const DONE: &str = "Done";
pub const DIDNT_HAPPEN: &str = "Didn't happen";
pub const NOT_YET: &str = "Not yet";

/// What one tick did with completion prompts (keys are `<run>:<ask>`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunPromptReport {
    /// Asks opened this tick (a run's first, or a moved run's next).
    pub opened: Vec<String>,
    /// Asks posted (bound) this tick.
    pub posted: Vec<String>,
    /// Asks closed because their run ended another way, is gone or moved.
    pub closed: Vec<String>,
    /// Asks whose post now shows the outcome.
    pub settled: Vec<String>,
}

/// Runs that are asked: live, and not own time.
fn asked(run: &Run) -> bool {
    matches!(
        run.status,
        RunStatus::Planned | RunStatus::Confirmed | RunStatus::AtRisk
    )
}

/// `Kalos, Kaling · Thu 10 Sep 20:00 · #1a2b3c4d`.
fn run_line(run: &Run, zone: chrono_tz::Tz) -> String {
    format!(
        "{} · {} {} · `#{}`",
        format_bosses(&run.bosses),
        local_day(run.datetime, zone),
        local_time(run.datetime, zone),
        short_id(&run.id)
    )
}

/// The open ask's layout: the run and its three buttons.
pub fn prompt_components(run: &Run, ask: u32, zone: chrono_tz::Tz) -> Vec<Component> {
    let id = |outcome| {
        ButtonId::RunPrompt {
            outcome,
            run_id: run.id.clone(),
            ask,
        }
        .custom_id()
    };
    vec![container(
        INK_BLUE,
        vec![
            text(format!(
                "🏁 **Did this run happen?**\n{}",
                run_line(run, zone)
            )),
            action_row(vec![
                button(ButtonStyle::Success, DONE, id(PromptOutcome::Done), false),
                button(
                    ButtonStyle::Danger,
                    DIDNT_HAPPEN,
                    id(PromptOutcome::DidntHappen),
                    false,
                ),
                button(
                    ButtonStyle::Secondary,
                    NOT_YET,
                    id(PromptOutcome::NotYet),
                    false,
                ),
            ]),
        ],
    )]
}

/// What a closed ask's post says, e.g. `🏁 Done · marked by <@1001>`.
pub fn outcome_text(prompt: &RunPrompt, run: Option<&Run>, zone: chrono_tz::Tz) -> String {
    let by = prompt
        .decided_by
        .as_deref()
        .map_or_else(String::new, |user| format!(" · marked by <@{user}>"));
    match prompt.outcome {
        // A press whose settle lost to a cancel or move elsewhere shows what
        // the run became instead.
        Some(PromptOutcome::Done)
            if run.is_none_or(|run| {
                matches!(run.status, RunStatus::Done) || run.status.is_live()
            }) =>
        {
            format!("🏁 Done{by}")
        }
        Some(PromptOutcome::DidntHappen)
            if run.is_none_or(|run| {
                matches!(run.status, RunStatus::Cancelled) || run.status.is_live()
            }) =>
        {
            format!("Didn't happen{by}")
        }
        Some(PromptOutcome::NotYet) => {
            let again = prompt
                .decided_at
                .map(|at| at + ASK_AGAIN_AFTER)
                .map(|at| {
                    if at < prompt.cutoff_at {
                        format!(" · asking again at {}", local_time(at, zone))
                    } else {
                        format!(
                            " · will be marked done at {}",
                            local_time(prompt.cutoff_at, zone)
                        )
                    }
                })
                .unwrap_or_default();
            format!("⏳ Not yet{by}{again}")
        }
        Some(PromptOutcome::AutoDone) => "🏁 Marked done automatically".to_owned(),
        Some(PromptOutcome::Moved) => "This run moved; this prompt is closed.".to_owned(),
        Some(PromptOutcome::Closed | PromptOutcome::Done | PromptOutcome::DidntHappen) | None => {
            match run.map(|run| run.status) {
                Some(RunStatus::Done) => "🏁 Done".to_owned(),
                Some(RunStatus::Cancelled) => "Cancelled".to_owned(),
                Some(status) => format!("Closed · {}", status_label(status)),
                None => "Closed".to_owned(),
            }
        }
    }
}

/// A closed ask's layout: its outcome above the run, no buttons.
pub fn outcome_components(
    prompt: &RunPrompt,
    run: Option<&Run>,
    zone: chrono_tz::Tz,
) -> Vec<Component> {
    let colour = match prompt.outcome {
        Some(PromptOutcome::Done | PromptOutcome::AutoDone) => SETTLED_GREEN,
        Some(PromptOutcome::DidntHappen) => AT_RISK_RED,
        Some(PromptOutcome::NotYet) => WAITING_AMBER,
        _ => INK_BLUE,
    };
    let mut body = outcome_text(prompt, run, zone);
    if let Some(run) = run {
        body.push('\n');
        body.push_str(&subtext(&run_line(run, zone)));
    }
    vec![container(colour, vec![text(body)])]
}

impl<S, I, T, A> Delivery<'_, S, I, T, A>
where
    S: ScheduleStore
        + DeliveryJournal
        + NoticeOutbox
        + Checkpoints
        + ProposalStore
        + super::cards::ReminderCardStore
        + RunPromptStore
        + Sync,
    I: IdSource,
    T: DiscordTransport,
    A: AlertSink,
{
    fn prompt_alert(&self, detail: String, now: DateTime<Utc>) {
        let alert = AdminAlert::RunPromptFailed { detail };
        if self.throttle().admit(&alert, now) {
            self.alerts.alert(alert);
        }
    }

    /// The run's completion timing at the live run lengths.
    fn completion_plan(&self, run: &Run) -> Result<CompletionPlan, DateOutOfRange> {
        let minutes = run_minutes(
            &self.config.run_lengths,
            self.cards.catalog.as_deref(),
            &run.bosses,
        );
        plan(run, minutes, &self.config.policy)
    }

    /// A press whose ask was claimed but whose settle was lost (a crash or
    /// store failure after the claim): settle it now as the presser, if the
    /// run is still live and at the end the ask was planned from. The press's
    /// request id makes this a replay when the settle had landed.
    async fn finish_pressed(&mut self, run: &Run, ends_at: DateTime<Utc>, now: DateTime<Utc>) {
        let latest = match self.store.latest_run_prompt(run.id.clone()).await {
            Ok(Some(latest)) => latest,
            Ok(None) => return,
            Err(error) => return self.prompt_alert(format!("read {}: {error}", run.id), now),
        };
        let status = match latest.outcome {
            Some(PromptOutcome::Done) => RunStatus::Done,
            Some(PromptOutcome::DidntHappen) => RunStatus::Cancelled,
            _ => return,
        };
        let Some(user) = latest
            .decided_by
            .clone()
            .filter(|_| latest.ends_at == ends_at)
        else {
            return;
        };
        let origin = Origin::new(Actor::member(user.clone()), Surface::Discord).with_request_id(
            pressed_request_id(
                &run.id,
                latest.ask,
                latest.outcome.unwrap_or(PromptOutcome::Done),
            ),
        );
        let settle = SettleRun {
            run_id: run.id.clone(),
            status,
            datetime: run.datetime,
            user,
            staff: true,
        };
        let policy = self.config.policy.reminders.clone();
        match self
            .service(now)
            .as_origin(origin)
            .settle_run(settle, &policy)
            .await
        {
            Ok(_) | Err(SchedulerError::AlreadyApplied { .. }) => {}
            Err(error) => self.prompt_alert(format!("settle {}: {error}", run.id), now),
        }
    }

    /// Mark every live run past its cutoff done, as Kanade, without a notice
    /// (own-time runs at the reset); its open ask closes as automatic. A
    /// failing run is alerted and retried next tick.
    ///
    /// # Errors
    /// The schedule could not be read.
    pub(super) async fn complete_runs(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, DeliveryError> {
        let runs = self.store.load(&Scope::All).await?.runs;
        let mut done = Vec::new();
        for run in runs.iter().filter(|run| run.status.is_live()) {
            let plan = match self.completion_plan(run) {
                Ok(plan) => plan,
                Err(error) => {
                    self.prompt_alert(format!("plan {}: {error}", run.id), now);
                    continue;
                }
            };
            if asked(run) {
                self.finish_pressed(run, plan.ends_at, now).await;
            }
            let cutoff = plan.cutoff_at;
            if now < cutoff {
                continue;
            }
            let origin = Origin::new(Actor::system("delivery"), Surface::DeliveryTick)
                .with_request_id(auto_request_id(&run.id, now));
            match self
                .service(now)
                .as_origin(origin)
                .finish_run(&run.id)
                .await
            {
                Ok(true) => done.push(run.id.clone()),
                Ok(false) | Err(SchedulerError::AlreadyApplied { .. }) => continue,
                Err(error) => {
                    self.prompt_alert(format!("finish {}: {error}", run.id), now);
                    continue;
                }
            }
            self.close_open_ask(&run.id, PromptOutcome::AutoDone, None, now)
                .await;
        }
        Ok(done)
    }

    /// Close the run's open ask, if any; `true` when this call closed it.
    async fn close_open_ask(
        &self,
        run_id: &str,
        outcome: PromptOutcome,
        next: Option<RunPrompt>,
        now: DateTime<Utc>,
    ) -> bool {
        let latest = match self.store.latest_run_prompt(run_id.to_owned()).await {
            Ok(latest) => latest,
            Err(error) => {
                self.prompt_alert(format!("read {run_id}: {error}"), now);
                return false;
            }
        };
        let Some(open) = latest.filter(RunPrompt::is_open) else {
            return false;
        };
        let close = PromptClose {
            outcome,
            decided_by: None,
            at: now,
            next,
        };
        match self
            .store
            .close_run_prompt(open.run_id.clone(), open.ask, close)
            .await
        {
            Ok(closed) => closed,
            Err(error) => {
                self.prompt_alert(format!("close {}: {error}", open.key()), now);
                false
            }
        }
    }

    /// Plan, post and settle completion prompts. A store failure is alerted
    /// and the next tick retries; lease loss or a journal backend failure
    /// aborts the tick.
    ///
    /// # Errors
    /// [`DeliveryError`] from the schedule read or the journal.
    pub(super) async fn run_prompts_in(
        &self,
        lease: &Lease,
        now: DateTime<Utc>,
    ) -> Result<RunPromptReport, DeliveryError> {
        let mut report = RunPromptReport::default();
        let snapshot = self.store.load(&Scope::All).await?;
        let runs: BTreeMap<&str, &Run> = snapshot
            .runs
            .iter()
            .map(|run| (run.id.as_str(), run))
            .collect();
        let open = match self.store.open_run_prompts().await {
            Ok(open) => open,
            Err(error) => {
                self.prompt_alert(format!("read: {error}"), now);
                return Ok(report);
            }
        };
        self.retire_or_replan(&open, &runs, now, &mut report).await;
        self.open_first_asks(&snapshot.runs, now, &mut report).await;
        match self.store.open_run_prompts().await {
            Ok(open) => {
                for prompt in open {
                    let Some(run) = runs.get(prompt.run_id.as_str()) else {
                        continue;
                    };
                    let due = prompt.message_id.is_none()
                        && prompt.due_at <= now
                        && now < prompt.cutoff_at
                        && asked(run);
                    if due && self.post_prompt(lease, &prompt, run, now).await? {
                        report.posted.push(prompt.key());
                    }
                }
            }
            Err(error) => self.prompt_alert(format!("read: {error}"), now),
        }
        match self.store.unsettled_run_prompts().await {
            Ok(unsettled) => {
                for prompt in unsettled {
                    let run = runs.get(prompt.run_id.as_str()).copied();
                    if self.settle_prompt(&prompt, run, now).await {
                        report.settled.push(prompt.key());
                    }
                }
            }
            Err(error) => self.prompt_alert(format!("read: {error}"), now),
        }
        Ok(report)
    }

    /// Open asks whose run is no longer asked (ended another way, own time,
    /// gone) close; a moved run's ask closes and its next one is planned
    /// from the new end, if it still gets one.
    async fn retire_or_replan(
        &self,
        open: &[RunPrompt],
        runs: &BTreeMap<&str, &Run>,
        now: DateTime<Utc>,
        report: &mut RunPromptReport,
    ) {
        for prompt in open {
            let run = runs.get(prompt.run_id.as_str()).filter(|run| asked(run));
            let (outcome, next) = match run {
                None => (PromptOutcome::Closed, None),
                Some(run) => {
                    let plan = match self.completion_plan(run) {
                        Ok(plan) => plan,
                        Err(error) => {
                            self.prompt_alert(format!("plan {}: {error}", run.id), now);
                            continue;
                        }
                    };
                    if plan.ends_at == prompt.ends_at {
                        continue;
                    }
                    let next = plan.prompt_at.map(|due| {
                        RunPrompt::open(
                            run.id.clone(),
                            prompt.ask + 1,
                            plan.ends_at,
                            due,
                            plan.cutoff_at,
                        )
                    });
                    (PromptOutcome::Moved, next)
                }
            };
            let next_key = next.as_ref().map(RunPrompt::key);
            let close = PromptClose {
                outcome,
                decided_by: None,
                at: now,
                next,
            };
            match self
                .store
                .close_run_prompt(prompt.run_id.clone(), prompt.ask, close)
                .await
            {
                Ok(true) => {
                    report.closed.push(prompt.key());
                    report.opened.extend(next_key);
                }
                Ok(false) => {}
                Err(error) => self.prompt_alert(format!("close {}: {error}", prompt.key()), now),
            }
        }
    }

    /// Open the first ask of every asked run whose prompt is due and not yet
    /// cut off. A run already asked keeps its asks: one closed by a press
    /// or automatically is never asked again unless the run moves (a new
    /// end), so a run reopened by hand is left to its cutoff.
    async fn open_first_asks(
        &self,
        runs: &[Run],
        now: DateTime<Utc>,
        report: &mut RunPromptReport,
    ) {
        for run in runs.iter().filter(|run| asked(run)) {
            let plan = match self.completion_plan(run) {
                Ok(plan) => plan,
                Err(error) => {
                    self.prompt_alert(format!("plan {}: {error}", run.id), now);
                    continue;
                }
            };
            let Some(due) = plan
                .prompt_at
                .filter(|due| *due <= now && now < plan.cutoff_at)
            else {
                continue;
            };
            let latest = match self.store.latest_run_prompt(run.id.clone()).await {
                Ok(latest) => latest,
                Err(error) => {
                    self.prompt_alert(format!("read {}: {error}", run.id), now);
                    continue;
                }
            };
            let ask = match latest {
                None => 0,
                // A new end, or a run live again after its ask closed because
                // it moved or ended another way (F8), is asked again.
                Some(last)
                    if !last.is_open()
                        && (last.ends_at != plan.ends_at
                            || matches!(
                                last.outcome,
                                Some(PromptOutcome::Moved | PromptOutcome::Closed)
                            )) =>
                {
                    last.ask + 1
                }
                Some(_) => continue,
            };
            let prompt = RunPrompt::open(run.id.clone(), ask, plan.ends_at, due, plan.cutoff_at);
            let key = prompt.key();
            match self.store.create_run_prompt(prompt).await {
                Ok(()) => report.opened.push(key),
                Err(error) => self.prompt_alert(format!("open {key}: {error}"), now),
            }
        }
    }

    /// Post one due ask; `true` once bound.
    async fn post_prompt(
        &self,
        lease: &Lease,
        prompt: &RunPrompt,
        run: &Run,
        now: DateTime<Utc>,
    ) -> Result<bool, DeliveryError> {
        let channel_id = match choose_channel(
            run.channel_id.as_deref(),
            self.config.post_channel_id.as_deref(),
            self.channels,
        ) {
            ChannelChoice::Requested(channel_id) | ChannelChoice::Fallback { channel_id, .. } => {
                channel_id
            }
            ChannelChoice::Unavailable => return Ok(false),
        };
        let message = OutgoingMessage::v2(
            prompt_components(run, prompt.ask, self.config.policy.zone()),
            mentions::none(),
        );
        let intent = NotificationIntent {
            effect: EffectKind::Notice(RUN_PROMPT_EFFECT.to_owned()),
            effect_context: vec![prompt.run_id.clone(), prompt.ask.to_string()],
            channel_id: channel_id.clone(),
            targets: Vec::new(),
            mentions: Vec::new(),
            content: IntentContent::Plain,
            warnings: Vec::new(),
        };
        let source = format!("run-prompt:{}", prompt.key());
        // A post bound before its message id was recorded (a crash between
        // bind and record): recover it from the journal, never post again.
        match self.store.bound_source(&source, 0).await {
            Ok(Some(receipt)) => {
                if let Err(error) = self
                    .store
                    .set_run_prompt_message(
                        prompt.run_id.clone(),
                        prompt.ask,
                        receipt.channel_id,
                        receipt.message_id,
                    )
                    .await
                {
                    self.prompt_alert(format!("record post of {}: {error}", prompt.key()), now);
                }
                return Ok(false);
            }
            Ok(None) => {}
            Err(error) => return Err(error.into()),
        }
        let Some(mut operation) = self.admit().await else {
            return Ok(false);
        };
        let result = self
            .executor(lease)
            .execute_source_in_operation(&mut operation, &intent, &message, &source, 0, now)
            .await;
        let outcome = match result {
            Ok(Some(outcome)) => Ok(outcome),
            Ok(None) => {
                operation.settle();
                return Ok(false);
            }
            Err(failure) => Err(failure),
        };
        let outcome = match settle(outcome) {
            Ok(outcome) => outcome,
            Err(error) => {
                operation.settle();
                return Err(error);
            }
        };
        let bound = match outcome {
            SendOutcome::Bound(message_id) => {
                let stored = self
                    .store
                    .set_run_prompt_message(
                        prompt.run_id.clone(),
                        prompt.ask,
                        channel_id,
                        id_text(message_id),
                    )
                    .await;
                if let Err(error) = stored {
                    self.prompt_alert(format!("record post of {}: {error}", prompt.key()), now);
                }
                true
            }
            _ => false,
        };
        operation.settle();
        Ok(bound)
    }

    /// Edit a closed ask's post into its outcome; `true` once it shows it
    /// or is gone.
    async fn settle_prompt(
        &self,
        prompt: &RunPrompt,
        run: Option<&Run>,
        now: DateTime<Utc>,
    ) -> bool {
        let (Some(channel), Some(message)) = (
            prompt.channel_id.as_deref().and_then(parse_id),
            prompt.message_id.as_deref().and_then(parse_id),
        ) else {
            return self.mark_prompt_settled(prompt, now).await;
        };
        let Some(mut operation) = self.admit().await else {
            return false;
        };
        if !operation.begin() {
            return false;
        }
        let edit = MessageEdit::v2(
            outcome_components(prompt, run, self.config.policy.zone()),
            mentions::none(),
        );
        let outcome = self.transport.edit_message(channel, message, &edit).await;
        operation.settle();
        match outcome {
            Outcome::Delivered(())
            | Outcome::DefinitelyRejected(
                RejectionKind::UnknownMessage
                | RejectionKind::UnknownChannel
                | RejectionKind::MissingAccess
                | RejectionKind::MissingPermissions,
            ) => self.mark_prompt_settled(prompt, now).await,
            _ => false,
        }
    }

    async fn mark_prompt_settled(&self, prompt: &RunPrompt, now: DateTime<Utc>) -> bool {
        match self
            .store
            .settle_run_prompt_message(prompt.run_id.clone(), prompt.ask)
            .await
        {
            Ok(()) => true,
            Err(error) => {
                self.prompt_alert(format!("settle {}: {error}", prompt.key()), now);
                false
            }
        }
    }
}
