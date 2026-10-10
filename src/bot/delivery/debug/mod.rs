//! `/debug ping`, `/debug clear_test` and `/debug header` (v4
//! `DebugGroup.ping`/`clear_test`, `effects.delete_debug_message`): the
//! [`DebugCards`] port.
//!
//! A ping posts a card of that kind, prefixed `🧪 TEST — `, journalled as a
//! `debug_card` effect under its own operation (v4 `DedupePolicy.operation`:
//! every ping is a new post, a retried operation is not). It never touches
//! the run's reminder rows, card records or digest phrases. The channel is
//! `channel:`, else the configured test channel, else the default: a run's
//! home channel (else the post channel, as v4 `post_channel`), the invoking
//! channel for a sample, the post channel for the week's digest.
//!
//! Posted in the run's home channel, a run's card gets ✅/❌ and on bind is
//! registered for the run so reactions drive its RSVPs; day-of and countdown
//! ones are refreshed like real cards (v4 `_rebuild_test_card`). People
//! follow v4's `test` audience: named, pinged only at ping level `all`.
//! Anything else (another channel, a sample run, a digest) is a sandbox
//! card: same look, nobody pinged, no ✅/❌ seeded, never registered or
//! refreshed. Headers are the seed unless `header:rewrite` asks for a fresh,
//! unstored rewrite. `clear_test` deletes the channel's test cards of the
//! last 24 h and marks them cleared.

mod header;
mod sample;
mod store;
mod text;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use chrono::{DateTime, Utc};
use tokio::time::Instant;

pub use header::{code_span, verdict_text};
pub use sample::{SAMPLE_RUN_ID, sample_schedule};
pub use store::{DebugCardStore, PostedDebugCard};

use super::alerts::{AlertThrottle, LogAlerts};
use super::cards::{
    self, CardContext, CardKit, HeaderKind, PhraseKind, TrialOrigin, Verdict, fetch_art, local_day,
};
use super::executor::{Executor, SendOutcome};
use super::notice_text::QUIET_NOTE;
use super::pregen::PREGEN_DEADLINE;
use super::refresh::Now;
use crate::bot::commands::{
    DebugCards, HeaderNote, HeaderRequest, HeaderTrialKind, HeaderTrials, PingRequest, PortFuture,
    TestKind, TestPosted, TestReport, TestSubject,
};
use crate::bot::ids::parse_id;
use crate::bot::mentions;
use crate::bot::transport::{DiscordTransport, Outcome, OutgoingMessage, RejectionKind};
use crate::domain::members::Directory;
use crate::domain::model_log::RewriteStage;
use crate::domain::notify::{
    ChannelChoice, ChannelDirectory, DeliveryJournal, DeliveryTarget, EffectKind, IntentContent,
    NotificationIntent, PingKind, PlannedSend, SendDisposition, WeekReset, choose_channel,
    digest_inclusion, resolve_mentions, sandbox_kind,
};
use crate::domain::schedule::{Run, SchedulePolicy, ScheduleSnapshot};
use crate::domain::scheduler::{ScheduleStore, Scope};
use crate::domain::settings::MessageStyle;
use crate::domain::time::to_iso;

/// v4 `TEST_PREFIX`.
pub const TEST_PREFIX: &str = "🧪 TEST — ";
/// `/debug header`'s most tries per command.
pub const MAX_HEADER_TRIES: u8 = 5;

/// What a test card renders.
struct Shown<'a> {
    schedule: &'a ScheduleSnapshot,
    run: Option<&'a Run>,
    /// The digest's boss week.
    week: DateTime<Utc>,
    style: MessageStyle,
    /// A fresh rewrite; `None` is the seed.
    header: Option<&'a str>,
}

/// v4's `test` audience: the party, pinged only if they asked for all pings;
/// nobody while quiet.
pub fn test_mentions(members: &dyn Directory, run: &Run, quiet: bool) -> Vec<String> {
    if quiet {
        return Vec::new();
    }
    let mut users = resolve_mentions(members, &run.participants, &PingKind::Test);
    users.sort();
    users.dedup();
    users
}

pub struct DebugDesk<S, T> {
    pub store: Arc<S>,
    pub transport: Arc<T>,
    pub members: Arc<dyn Directory + Send + Sync>,
    pub channels: Arc<dyn ChannelDirectory + Send + Sync>,
    pub cards: CardKit,
    pub policy: SchedulePolicy,
    /// The tick's live quiet-mode and post-channel settings.
    pub quiet: Arc<AtomicBool>,
    pub post_channel: Arc<RwLock<Option<String>>>,
    /// `[discord] test_channel`: where pings without `channel:` post.
    pub test_channel: Option<String>,
    pub instance_id: String,
    pub now: Now,
    pub throttle: AlertThrottle,
}

impl<S, T> DebugDesk<S, T>
where
    S: ScheduleStore + DeliveryJournal + DebugCardStore + Send + Sync,
    T: DiscordTransport,
{
    fn reachable(&self, channel: Option<String>) -> Option<String> {
        channel.filter(|id| self.channels.is_reachable(id))
    }

    /// `channel:`, else the test channel (either must be reachable), else
    /// the subject's default; `None` when unreachable.
    fn target(&self, request: Option<&String>, default: Option<String>) -> Option<String> {
        match request.or(self.test_channel.as_ref()) {
            Some(channel) => self.reachable(Some(channel.clone())),
            None => default,
        }
    }

    async fn post_test(&self, request: &PingRequest) -> Result<TestReport, String> {
        let unreachable = TestReport {
            posted: TestPosted::Unreachable,
            header: None,
        };
        let now = (self.now)();
        let post = self
            .post_channel
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let invoked = self.reachable(request.invoked_in.clone());
        // The run's home channel (else the post channel), where a test card
        // is registered for the run; every other post is a sandbox.
        let (schedule, home, subject_id) = match &request.subject {
            TestSubject::Run(run_id) => {
                let schedule = self.load().await?;
                let run = schedule
                    .runs
                    .iter()
                    .find(|run| &run.id == run_id)
                    .ok_or_else(|| format!("run {run_id} vanished"))?;
                let home = match choose_channel(
                    run.channel_id.as_deref(),
                    post.as_deref(),
                    &*self.channels,
                ) {
                    ChannelChoice::Requested(channel)
                    | ChannelChoice::Fallback {
                        channel_id: channel,
                        ..
                    } => Some(channel),
                    ChannelChoice::Unavailable => None,
                };
                (Some(schedule), home, run_id.clone())
            }
            TestSubject::Sample(_) => (None, None, SAMPLE_RUN_ID.to_owned()),
            TestSubject::Week => (Some(self.load().await?), None, String::new()),
        };
        let default = match &request.subject {
            TestSubject::Run(_) => home.clone(),
            TestSubject::Sample(_) => invoked,
            TestSubject::Week => self.reachable(post).or(invoked),
        };
        let Some(channel_id) = self.target(request.channel.as_ref(), default) else {
            return Ok(unreachable);
        };
        let schedule = match (&request.subject, schedule) {
            (TestSubject::Sample(spec), _) => {
                sample::sample_schedule(spec, &channel_id, &self.policy, now)?
            }
            (_, Some(schedule)) => schedule,
            (_, None) => return Err("no schedule".into()),
        };
        let run = schedule.runs.iter().find(|run| run.id == subject_id);
        let week = match (request.kind, run) {
            (TestKind::Digest, Some(run)) => run.week_start,
            _ => self
                .reset()
                .current_week(now)
                .map_err(|error| error.to_string())?,
        };
        let registered = matches!(request.subject, TestSubject::Run(_))
            && request.kind != TestKind::Digest
            && home.as_deref() == Some(channel_id.as_str());
        let (header, note) = self.header(request, run).await;
        let quiet = self.quiet.load(Ordering::Relaxed);
        let mentioned = run
            .map(|run| test_mentions(&*self.members, run, quiet))
            .unwrap_or_default();
        let shown = Shown {
            schedule: &schedule,
            run,
            week,
            style: request.style.unwrap_or_else(|| self.cards.style()),
            header: header.as_deref(),
        };
        let mut message = self
            .render(
                &shown,
                request.kind,
                &mentioned,
                &request.requested_by,
                quiet,
            )
            .await?;
        // A sandbox post looks like the real card but pings nobody.
        let mentions = if registered {
            mentioned
        } else {
            message.allowed_mentions = mentions::none();
            Vec::new()
        };
        let (run_id, stored_kind) = match &request.subject {
            TestSubject::Week => (
                format!("week:{}", to_iso(&week).map_err(|error| error.to_string())?),
                request.kind.as_str(),
            ),
            _ => (subject_id, request.kind.as_str()),
        };
        let stored_kind = if registered {
            stored_kind.to_owned()
        } else {
            sandbox_kind(stored_kind)
        };
        let intent = NotificationIntent {
            effect: EffectKind::DebugCard,
            effect_context: vec![run_id.clone(), stored_kind.clone()],
            channel_id: channel_id.clone(),
            targets: vec![DeliveryTarget::DebugCard {
                run_id,
                kind: stored_kind,
            }],
            mentions,
            content: IntentContent::Plain,
            warnings: Vec::new(),
        };
        let posted = match self.send(intent, &message, now).await? {
            true if registered => TestPosted::Posted { channel_id },
            true => TestPosted::Sandboxed { channel_id },
            false => TestPosted::Unconfirmed,
        };
        Ok(TestReport {
            posted,
            header: note,
        })
    }

    async fn load(&self) -> Result<ScheduleSnapshot, String> {
        self.store
            .load(&Scope::All)
            .await
            .map_err(|error| format!("store: {error}"))
    }

    fn reset(&self) -> WeekReset {
        WeekReset {
            zone: self.policy.zone(),
            weekday: self.policy.reset_weekday,
            time: self.policy.reset_time,
        }
    }

    /// `header:rewrite`: a fresh rewrite of the kind's header, never stored;
    /// the seed (`None`) on any failure.
    async fn header(
        &self,
        request: &PingRequest,
        run: Option<&Run>,
    ) -> (Option<String>, Option<HeaderNote>) {
        let kind = match request.kind {
            TestKind::DayOf => HeaderKind::DayOf,
            TestKind::Countdown60 | TestKind::Countdown15 => {
                HeaderKind::Phrase(PhraseKind::Countdown)
            }
            TestKind::Digest => HeaderKind::Phrase(PhraseKind::Digest),
            TestKind::Amend | TestKind::Decline => return (None, None),
        };
        if !request.rewrite {
            return (None, None);
        }
        let catalog = self.cards.catalog.as_deref();
        let context = match run {
            Some(run) => format!("/debug ping {} · run {}", request.kind.as_str(), run.id),
            None => format!("/debug ping {}", request.kind.as_str()),
        };
        let origin = TrialOrigin::new(RewriteStage::Debug, context);
        let trial = self
            .cards
            .heading
            .trial(kind, catalog, PREGEN_DEADLINE, &origin)
            .await;
        let note = HeaderNote {
            rewritten: trial.verdict == Verdict::Accepted,
            reason: header::verdict_text(&trial),
        };
        if !note.rewritten {
            return (None, Some(note));
        }
        let line = match (kind, run) {
            (HeaderKind::DayOf, Some(run)) => trial
                .line
                .replace("{day}", &local_day(run.datetime, self.policy.zone())),
            _ => trial.line,
        };
        (Some(line), Some(note))
    }

    /// Claim, post and bind one test card: `false` when unconfirmed.
    async fn send(
        &self,
        intent: NotificationIntent,
        message: &OutgoingMessage,
        now: DateTime<Utc>,
    ) -> Result<bool, String> {
        let lease = self
            .store
            .begin_lease(&self.instance_id, EffectKind::DebugCard.as_str(), now)
            .await
            .map_err(|error| format!("journal: {error}"))?;
        let executor = Executor {
            journal: &*self.store,
            transport: &*self.transport,
            alerts: &LogAlerts,
            throttle: &self.throttle,
            lease: &lease,
        };
        let send = PlannedSend {
            intent,
            disposition: SendDisposition::Send,
        };
        let outcome = executor.execute(&send, message, None, None, now).await;
        // Best effort: an unended lease is orphaned by restart recovery.
        let _ = self.store.end_lease(&lease, now).await;
        match outcome {
            Ok(SendOutcome::Bound(_)) => Ok(true),
            Ok(SendOutcome::Unavailable(detail)) => Err(detail),
            // Maybe posted, refused or never sent: say so, as v4 did.
            Ok(_) => Ok(false),
            // Discord took it but the journal write after failed: it may be up.
            Err(failure) if failure.maybe_delivered => Ok(false),
            Err(failure) => Err(format!("journal: {}", failure.error)),
        }
    }

    /// The real message of `kind`, prefixed; plain texts carry v4's quiet
    /// line while quiet (cards never announce it).
    async fn render(
        &self,
        shown: &Shown<'_>,
        kind: TestKind,
        mentioned: &[String],
        requested_by: &str,
        quiet: bool,
    ) -> Result<OutgoingMessage, String> {
        let ctx = CardContext {
            schedule: shown.schedule,
            attendance: self.policy.attendance,
            zone: self.policy.zone(),
            quiet,
            members: &*self.members,
            catalog: self.cards.catalog.as_deref(),
            style: shown.style,
            marks: &self.cards.marks,
            v2: Some(&self.cards.v2),
        };
        let card_content = match (kind, shown.run) {
            (TestKind::Digest, _) => Some(IntentContent::Digest {
                week_start: shown.week,
                inclusion: digest_inclusion(
                    &shown.schedule.runs,
                    shown.week,
                    ctx.zone,
                    self.cards
                        .run_ends(&self.policy)
                        .as_ref()
                        .map(|ends| (ends, (self.now)())),
                )
                .map_err(|error| error.to_string())?,
            }),
            (TestKind::DayOf, Some(run)) => Some(IntentContent::DayOf {
                run_ids: vec![run.id.clone()],
            }),
            (TestKind::Countdown60 | TestKind::Countdown15, Some(run)) => {
                Some(IntentContent::Countdown {
                    run_id: run.id.clone(),
                    minutes: if kind == TestKind::Countdown60 {
                        60
                    } else {
                        15
                    },
                })
            }
            _ => None,
        };
        // The digest names nobody (v4).
        let mentioned = if kind == TestKind::Digest {
            &[]
        } else {
            mentioned
        };
        if let Some(content) = card_content {
            let card = cards::build(&content, &ctx, shown.header, mentioned)
                .ok_or_else(|| format!("no {} card", kind.as_str()))?;
            let card = card.prefixed(TEST_PREFIX);
            let pictures = fetch_art(self.cards.art.as_ref(), &card, true).await;
            return Ok(card.message(mentioned, &pictures));
        }
        let run = shown.run.ok_or("no run to render")?;
        let text = if kind == TestKind::Decline {
            let name = self.members.display_name(requested_by).unwrap_or_else(|| {
                if quiet {
                    cards::UNNAMED.to_owned()
                } else {
                    format!("<@{requested_by}>")
                }
            });
            text::decline_text(&ctx, run, mentioned, requested_by, &name)
        } else {
            text::amend_text(&ctx, run, mentioned)
        };
        let mut content = format!("{TEST_PREFIX}{text}");
        if quiet {
            // v4 `formatting.quieted` for a message without an embed.
            content = format!("{content}\n_{QUIET_NOTE}_");
        }
        Ok(OutgoingMessage {
            content: Some(content),
            embeds: Vec::new(),
            allowed_mentions: mentions::allow_users(mentioned),
            reply_to: None,
            attachments: Vec::new(),
            components: Vec::new(),
        })
    }

    /// `/debug header`: `tries` sequential rewrites, posted as one public
    /// report with no mentions; nothing is stored.
    async fn header_trials(&self, request: &HeaderRequest) -> Result<HeaderTrials, String> {
        if !self.cards.heading.enabled() {
            return Ok(HeaderTrials::Disabled);
        }
        let default = self.reachable(request.invoked_in.clone());
        let Some(channel_id) = self.target(request.channel.as_ref(), default) else {
            return Ok(HeaderTrials::Unreachable);
        };
        let Some(channel) = parse_id(&channel_id) else {
            return Ok(HeaderTrials::Unreachable);
        };
        let kind = match request.kind {
            HeaderTrialKind::DayOf => HeaderKind::DayOf,
            HeaderTrialKind::Countdown => HeaderKind::Phrase(PhraseKind::Countdown),
            HeaderTrialKind::Digest => HeaderKind::Phrase(PhraseKind::Digest),
        };
        let catalog = self.cards.catalog.as_deref();
        let count = request.tries.clamp(1, MAX_HEADER_TRIES);
        let mut tries = Vec::new();
        for index in 1..=count {
            let origin = TrialOrigin::new(
                RewriteStage::Debug,
                format!(
                    "/debug header {} · try {index}/{count}",
                    request.kind.as_str()
                ),
            );
            let started = Instant::now();
            let trial = self
                .cards
                .heading
                .trial(kind, catalog, PREGEN_DEADLINE, &origin)
                .await;
            tries.push((trial, started.elapsed()));
        }
        let accepted = tries
            .iter()
            .filter(|(trial, _)| trial.verdict == Verdict::Accepted)
            .count();
        let message = OutgoingMessage {
            content: Some(header::report(request.kind, &tries)),
            embeds: Vec::new(),
            allowed_mentions: mentions::none(),
            reply_to: None,
            attachments: Vec::new(),
            components: Vec::new(),
        };
        Ok(
            if self
                .transport
                .create_message(channel, &message)
                .await
                .is_delivered()
            {
                HeaderTrials::Posted {
                    channel_id,
                    accepted,
                }
            } else {
                HeaderTrials::Unconfirmed
            },
        )
    }

    /// Delete each test card in `channel_id` since `since`; a message
    /// already gone counts as deleted. `(deleted, failed)`.
    async fn clear_tests(
        &self,
        channel_id: &str,
        since: DateTime<Utc>,
    ) -> Result<(usize, usize), String> {
        let posted = self
            .store
            .debug_cards_in(channel_id, since)
            .await
            .map_err(|error| format!("store: {error}"))?;
        let (mut deleted, mut failed) = (0, 0);
        for card in posted {
            let (Some(channel), Some(message)) =
                (parse_id(&card.channel_id), parse_id(&card.message_id))
            else {
                failed += 1;
                continue;
            };
            let gone = matches!(
                self.transport.delete_message(channel, message).await,
                Outcome::Delivered(()) | Outcome::DefinitelyRejected(RejectionKind::UnknownMessage)
            );
            if gone
                && self
                    .store
                    .clear_debug_card(&card.message_id, (self.now)())
                    .await
                    .is_ok()
            {
                deleted += 1;
            } else {
                failed += 1;
            }
        }
        Ok((deleted, failed))
    }
}

impl<S, T> DebugCards for DebugDesk<S, T>
where
    S: ScheduleStore + DeliveryJournal + DebugCardStore + Send + Sync + 'static,
    T: DiscordTransport + 'static,
{
    fn ping(&self, request: PingRequest) -> PortFuture<'_, Result<TestReport, String>> {
        Box::pin(async move { self.post_test(&request).await })
    }

    fn clear(
        &self,
        channel_id: String,
        since: DateTime<Utc>,
    ) -> PortFuture<'_, Result<(usize, usize), String>> {
        Box::pin(async move { self.clear_tests(&channel_id, since).await })
    }

    fn headers(&self, request: HeaderRequest) -> PortFuture<'_, Result<HeaderTrials, String>> {
        Box::pin(async move { self.header_trials(&request).await })
    }
}
