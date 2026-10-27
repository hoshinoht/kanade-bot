//! Persona-voiced reminder headers. Day-of keeps its original rewrite contract;
//! countdown and digest accept a short persona phrase that states no facts.
//! Rewrites run ahead of the send (`delivery::pregen`); a send never calls
//! the model and uses the stored line or the seed. A day-of or countdown
//! rewrite may carry estimate context ([`HeaderContext`]); its line then goes
//! through [`accept_header_with`] and may hold `{cN}` placeholders that only
//! [`fill_slots`] turns into names.

mod context;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

pub use context::{
    HeaderContext, HeaderSwitchSource, HeaderSwitches, MAX_SLOTS, SlotFill, Teasable, fill_slots,
    resolve_heading,
};

use kanade_domain::catalog::BossTable;
use kanade_domain::model_log::{RewriteKind, RewriteStage};
use kanade_domain::notify::HeaderSlots;
use kanade_voice::nudge::{
    NudgeRewriter, Rejection, RewriteAttempt, RewriteDetail, RewriteFailure, RewritePrompt,
    SharedRewriteSink, SharedRewriter, SlotRejection, WordFilter, WordSource, accept_rewrite_with,
    failure_verdict, has_format_char, has_invite, has_markup, safe_value, strip_slots,
};
use kanade_voice::persona::{CompiledPersona, MAX_NUDGE_CHARS, NudgeMood, check_nudge_line};

/// v4's heading, with the day left for after the rewrite.
pub const DAY_OF_HEADING_SEED: &str = "Today — {day}";
pub const COUNTDOWN_PHRASE_SEED: &str = "Onward!";
pub const DIGEST_PHRASE_SEED: &str = "Let's go!";
const DAY: &str = "{day}";
/// A header phrase: a short line, never a sentence of news.
const MAX_PHRASE_CHARS: usize = 60;
const MAX_PHRASE_WORDS: usize = 8;

const FACT_WORDS: &[&str] = &[
    "today",
    "tonight",
    "tomorrow",
    "yesterday",
    "week",
    "day",
    "date",
    "time",
    "hour",
    "minute",
    "second",
    "morning",
    "afternoon",
    "evening",
    "night",
    "noon",
    "midnight",
    "am",
    "pm",
    "next",
    "last",
    "now",
    "later",
    "soon",
    "monday",
    "mon",
    "tuesday",
    "tue",
    "tues",
    "wednesday",
    "wed",
    "thursday",
    "thu",
    "thur",
    "thurs",
    "friday",
    "fri",
    "saturday",
    "sat",
    "sunday",
    "sun",
    "january",
    "jan",
    "february",
    "feb",
    "march",
    "mar",
    "april",
    "apr",
    "may",
    "june",
    "jun",
    "july",
    "jul",
    "august",
    "aug",
    "september",
    "sep",
    "sept",
    "october",
    "oct",
    "november",
    "nov",
    "december",
    "dec",
    "yes",
    "no",
    "confirm",
    "confirmed",
    "confirmation",
    "unconfirmed",
    "decline",
    "declined",
    "out",
    "pending",
    "answered",
    "answer",
    "ready",
    "set",
    "done",
    "cleared",
    "complete",
    "completed",
    "planned",
    "risk",
    "cancel",
    "cancelled",
    "canceled",
    "waiting",
    "everyone",
    // Times and counts written out ("quarter past eight", "two runs").
    "o'clock",
    "oclock",
    "quarter",
    "half",
    "past",
    "till",
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
    "twenty",
    "thirty",
    "forty",
    "fifty",
    "first",
    "twice",
    "dozen",
];

/// The guild default persona (bundle, no member profile), if chat has one.
pub type PersonaSource = Arc<dyn Fn() -> Option<CompiledPersona> + Send + Sync>;

/// A header rewritten ahead of its send: the line (the day filled in for
/// day-of; `{cN}` left for [`fill_slots`]), where it came from, the
/// failure code behind a seed and the slot map of a line written with
/// estimate context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub line: String,
    pub source: HeadingSource,
    pub code: Option<&'static str>,
    pub slots: Option<HeaderSlots>,
}

kanade_domain::spelled! {
    /// Where a heading came from, for the `day_of_heading` log line.
    HeadingSource {
        Rewrite => "rewrite",
        Seed => "seed",
    }
}

kanade_domain::spelled! {
    PhraseKind {
        Countdown => "countdown",
        Digest => "digest",
    }
}

impl PhraseKind {
    pub fn seed(self) -> &'static str {
        match self {
            Self::Countdown => COUNTDOWN_PHRASE_SEED,
            Self::Digest => DIGEST_PHRASE_SEED,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhraseRejection {
    /// Fails the day-of line rules first.
    UnsafeLine(Rejection),
    FactualTerm,
    CatalogTerm,
    /// No word at all (only emoji or punctuation).
    NoWords,
    /// Over [`MAX_PHRASE_WORDS`] words or [`MAX_PHRASE_CHARS`] characters.
    TooLong,
}

/// Which header a rewrite is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderKind {
    DayOf,
    Phrase(PhraseKind),
}

impl HeaderKind {
    fn log_kind(self) -> RewriteKind {
        match self {
            Self::DayOf => RewriteKind::DayOf,
            Self::Phrase(PhraseKind::Countdown) => RewriteKind::Countdown,
            Self::Phrase(PhraseKind::Digest) => RewriteKind::Digest,
        }
    }
}

/// Where a trial runs and what it is for, for the Rewrites log: a card key,
/// a digest week or a `/debug` command, never a member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrialOrigin {
    pub stage: RewriteStage,
    pub context: Option<String>,
}

impl TrialOrigin {
    pub fn new(stage: RewriteStage, context: impl Into<String>) -> Self {
        Self {
            stage,
            context: Some(context.into()),
        }
    }
}

/// A header line written with estimate context that breaks a placeholder,
/// name, digit or fill rule (D5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotRule {
    /// A `{cN}` that was not offered.
    UnknownSlot,
    /// An offered placeholder used twice.
    DuplicateSlot,
    /// More than [`MAX_SLOTS`] placeholders, a tease slot included.
    TooManySlots,
    /// A party member's name, alias or IGN written out.
    RawName,
    /// A digit in a day-of line while context was sent.
    Digits,
    /// Over today's caps once the names are in.
    TooLongFilled,
    /// A name that can't be filled safely, or a filled line that fails the
    /// line rules.
    UnsafeFill,
}

impl SlotRule {
    /// At most 64 bytes (the Rewrites log `rule` CHECK, 0027).
    pub fn rule(self) -> &'static str {
        match self {
            Self::UnknownSlot => "unknown slot",
            Self::DuplicateSlot => "duplicate slot",
            Self::TooManySlots => "too many slots",
            Self::RawName => "raw name",
            Self::Digits => "digits with estimates",
            Self::TooLongFilled => "too long after fill",
            Self::UnsafeFill => "unsafe fill",
        }
    }
}

/// The outcome of one rewrite attempt, with the gate rule that refused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    /// Day-of line rules.
    Rejected(Rejection),
    /// Countdown/digest phrase rules.
    PhraseRejected(PhraseRejection),
    /// The estimate-context rules ([`accept_header_with`]).
    SlotRejected(SlotRule),
    Timeout,
    Failed(RewriteFailure),
    NoRewriter,
    NoPersona,
}

impl Verdict {
    /// The log reason (`rejected` for every gate rule).
    pub fn reason(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Rejected(_) | Self::PhraseRejected(_) | Self::SlotRejected(_) => "rejected",
            Self::Timeout => "timeout",
            Self::Failed(failure) => failure_reason(failure),
            Self::NoRewriter => "no_rewriter",
            Self::NoPersona => "no_persona",
        }
    }

    /// The gate rule that refused the line, if one did.
    pub fn rule(self) -> Option<&'static str> {
        match self {
            Self::Rejected(rejection)
            | Self::PhraseRejected(PhraseRejection::UnsafeLine(rejection)) => {
                Some(rejection.rule())
            }
            Self::PhraseRejected(PhraseRejection::FactualTerm) => Some("factual term"),
            Self::PhraseRejected(PhraseRejection::CatalogTerm) => Some("catalog term"),
            Self::PhraseRejected(PhraseRejection::NoWords) => Some("no words"),
            Self::PhraseRejected(PhraseRejection::TooLong) => Some("too long"),
            Self::SlotRejected(rule) => Some(rule.rule()),
            _ => None,
        }
    }
}

/// One rewrite attempt: the model's raw reply (if any), the line to use
/// (the accepted rewrite or the seed; day-of keeps `{day}`), the verdict,
/// what the call reported (its error code above all) and, for a line
/// accepted with estimate context, its slot map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub output: Option<String>,
    pub line: String,
    pub verdict: Verdict,
    pub detail: RewriteDetail,
    pub slots: Option<HeaderSlots>,
}

impl Trial {
    /// The specific failure behind an `unavailable`/`refused`/`misconfigured`
    /// verdict (`budget_exceeded`, `busy`, `shutdown`, …).
    pub fn code(&self) -> Option<&'static str> {
        self.detail.code
    }

    pub fn source(&self) -> HeadingSource {
        if self.verdict == Verdict::Accepted {
            HeadingSource::Rewrite
        } else {
            HeadingSource::Seed
        }
    }
}

/// The `day_of_heading` log reason for a rewriter failure; operator
/// settings stay distinguishable from outages.
pub fn failure_reason(failure: RewriteFailure) -> &'static str {
    failure_verdict(failure)
}

/// v4's heading for `day`.
pub fn seed_heading(day: &str) -> String {
    DAY_OF_HEADING_SEED.replace(DAY, day)
}

/// Accept a short persona phrase: the shared line rules, then no digits, no
/// date, time or status word and no catalog name; emoji, `~`, dashes and
/// ellipses are fine.
pub fn accept_phrase(
    output: &str,
    seed: &str,
    catalog: Option<&BossTable>,
) -> Result<String, PhraseRejection> {
    accept_phrase_with(output, seed, catalog, WordFilter::builtin())
}

/// As [`accept_phrase`], against the live effective deny-list.
pub fn accept_phrase_with(
    output: &str,
    seed: &str,
    catalog: Option<&BossTable>,
    words: &WordFilter,
) -> Result<String, PhraseRejection> {
    let line = accept_rewrite_with(output, seed, words).map_err(PhraseRejection::UnsafeLine)?;
    if line.chars().any(char::is_numeric) {
        return Err(PhraseRejection::FactualTerm);
    }
    let tokens = phrase_tokens(&line);
    if tokens.is_empty() {
        return Err(PhraseRejection::NoWords);
    }
    if tokens.len() > MAX_PHRASE_WORDS || line.chars().count() > MAX_PHRASE_CHARS {
        return Err(PhraseRejection::TooLong);
    }
    if tokens
        .iter()
        .any(|token| FACT_WORDS.contains(&token.as_str()))
    {
        return Err(PhraseRejection::FactualTerm);
    }
    if catalog.is_some_and(|catalog| names_catalog_entry(&tokens, catalog)) {
        return Err(PhraseRejection::CatalogTerm);
    }
    Ok(line)
}

/// A line accepted with estimate context: the template (`{cN}` unfilled)
/// and the placeholders it uses, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderLine {
    pub template: String,
    pub used: Vec<String>,
}

/// The header gate when estimate context was sent (D5). The offered `{cN}`
/// placeholders are checked first (each at most once, at most
/// [`MAX_SLOTS`] in all, a tease slot included; an unknown one refuses) and
/// taken out, then the stripped line must pass `kind`'s own
/// gate ([`accept_rewrite_with`] for day-of, [`accept_phrase_with`] for a
/// phrase); a day-of line may hold no digit. The line must not carry any
/// party name, alias or IGN raw. Filled with the IGNs, it must still pass
/// the line rules under today's caps. Persona pools keep
/// `check_nudge_line`, which refuses `{cN}`.
pub fn accept_header_with(
    output: &str,
    kind: HeaderKind,
    context: &HeaderContext,
    catalog: Option<&BossTable>,
    words: &WordFilter,
) -> Result<HeaderLine, Verdict> {
    let line = output.trim();
    let (stripped, used) = strip_slots(line, &context.offered()).map_err(|rejection| {
        Verdict::SlotRejected(match rejection {
            SlotRejection::Unknown => SlotRule::UnknownSlot,
            SlotRejection::Duplicate => SlotRule::DuplicateSlot,
        })
    })?;
    if used.len() > MAX_SLOTS {
        return Err(Verdict::SlotRejected(SlotRule::TooManySlots));
    }
    let seed = seed_of(kind);
    match kind {
        HeaderKind::DayOf => {
            accept_rewrite_with(&stripped, seed, words).map_err(Verdict::Rejected)?;
            if stripped.chars().any(char::is_numeric) {
                return Err(Verdict::SlotRejected(SlotRule::Digits));
            }
        }
        HeaderKind::Phrase(_) => {
            accept_phrase_with(&stripped, seed, catalog, words).map_err(Verdict::PhraseRejected)?;
        }
    }
    if context::names_any(line, &context.names) {
        return Err(Verdict::SlotRejected(SlotRule::RawName));
    }
    let mut filled = line.to_owned();
    for slot in &used {
        let Some(fill) = context.fills.iter().find(|fill| &fill.placeholder == slot) else {
            return Err(Verdict::SlotRejected(SlotRule::UnknownSlot));
        };
        if !safe_value(&fill.ign) {
            return Err(Verdict::SlotRejected(SlotRule::UnsafeFill));
        }
        filled = filled.replace(slot, &fill.ign);
    }
    let too_long = match kind {
        HeaderKind::DayOf => filled.chars().count() > MAX_NUDGE_CHARS,
        HeaderKind::Phrase(_) => {
            filled.chars().count() > MAX_PHRASE_CHARS
                || phrase_tokens(&filled).len() > MAX_PHRASE_WORDS
        }
    };
    if too_long {
        return Err(Verdict::SlotRejected(SlotRule::TooLongFilled));
    }
    if check_nudge_line(&filled).is_err()
        || has_markup(&filled)
        || has_format_char(&filled)
        || has_invite(&filled)
        || words.denied(&filled).is_some()
    {
        return Err(Verdict::SlotRejected(SlotRule::UnsafeFill));
    }
    Ok(HeaderLine {
        template: line.to_owned(),
        used,
    })
}

/// Lowercase words (letters with inner apostrophes); everything else
/// (punctuation, `~`, dashes, emoji) separates them.
fn phrase_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for character in line.chars() {
        if character.is_alphabetic() {
            word.extend(character.to_lowercase());
        } else if matches!(character, '\'' | '’') && !word.is_empty() {
            word.push('\'');
        } else if !word.is_empty() {
            tokens.push(std::mem::take(&mut word));
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}

fn names_catalog_entry(tokens: &[String], catalog: &BossTable) -> bool {
    let contains = |value: &str| {
        let words = value
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        !words.is_empty()
            && tokens
                .windows(words.len())
                .any(|window| window.iter().zip(&words).all(|(left, right)| left == right))
    };
    for difficulty in catalog.difficulties() {
        if contains(difficulty.label()) || contains(difficulty.letter()) {
            return true;
        }
    }
    catalog.bosses().iter().any(|boss| {
        contains(boss.short())
            || contains(boss.full())
            || boss.aliases().iter().any(|alias| contains(alias))
            || boss
                .difficulties()
                .iter()
                .any(|letter| contains(&boss.canonical(letter)))
    })
}

/// The rewriter and persona; either missing means the seed. `words` is the
/// live profanity list, read per rewrite (`None`: the built-in list); `log`
/// receives one Rewrites-log row per trial (`None`: nothing is logged).
#[derive(Clone, Default)]
pub struct HeadingRewrite {
    pub rewriter: Option<SharedRewriter>,
    pub persona: Option<PersonaSource>,
    pub words: Option<WordSource>,
    pub log: Option<SharedRewriteSink>,
}

impl std::fmt::Debug for HeadingRewrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeadingRewrite")
            .field("rewriter", &self.rewriter.is_some())
            .field("persona", &self.persona.is_some())
            .field("words", &self.words.is_some())
            .field("log", &self.log.is_some())
            .finish()
    }
}

impl HeadingRewrite {
    fn words(&self) -> Arc<WordFilter> {
        self.words.as_ref().map_or_else(
            || Arc::new(WordFilter::builtin().clone()),
            |source| source(),
        )
    }

    /// The prompt a rewrite of `kind` with `context` would send, if one
    /// would be attempted (digest phrases never carry context).
    pub fn prompt_with(
        &self,
        kind: HeaderKind,
        context: Option<&HeaderContext>,
    ) -> Option<RewritePrompt> {
        self.rewriter.as_ref()?;
        let persona = (self.persona.as_ref()?)()?;
        let context = context.filter(|_| kind != HeaderKind::Phrase(PhraseKind::Digest));
        Some(RewritePrompt::header(
            &persona,
            context.map_or(NudgeMood::Playful, |context| context.mood),
            seed_of(kind),
            context.map(|context| &context.facts),
        ))
    }

    /// Whether a pre-generation pass has anything to call.
    pub fn enabled(&self) -> bool {
        self.rewriter.is_some() && self.persona.is_some()
    }

    /// The heading for `day` (e.g. `Fri 25 Sep`), rewritten ahead of the
    /// send for the card under `key` by `stage` (a batch or a catch-up);
    /// never slower than `deadline` and ended by the worker's `stop` (a cut
    /// call fails with code `shutdown`). `context` adds the estimate block
    /// and its gate. Logs the source and the failure code, never the text.
    pub async fn choose(
        &self,
        day: &str,
        key: &str,
        stage: RewriteStage,
        deadline: Duration,
        stop: Option<&watch::Receiver<bool>>,
        context: Option<&HeaderContext>,
    ) -> Chosen {
        let origin = TrialOrigin::new(stage, key);
        let trial = self
            .trial_until(HeaderKind::DayOf, None, deadline, &origin, stop, context)
            .await;
        kanade_log::log!(
            info,
            "day_of_heading",
            stage = stage.as_str(),
            source = trial.source().as_str(),
            reason = trial.verdict.reason(),
            detail = trial.code(),
            estimates = context.is_some(),
        );
        Chosen {
            line: trial.line.replace(DAY, day),
            source: trial.source(),
            code: trial.code(),
            slots: trial.slots,
        }
    }

    /// A countdown or digest phrase rewritten ahead of the send for the card
    /// or digest under `key`. Its prompt contains only the persona, the
    /// code-owned seed and, for a countdown, the estimate `context`.
    #[allow(clippy::too_many_arguments)]
    pub async fn choose_phrase(
        &self,
        kind: PhraseKind,
        catalog: Option<&BossTable>,
        key: &str,
        stage: RewriteStage,
        deadline: Duration,
        stop: Option<&watch::Receiver<bool>>,
        context: Option<&HeaderContext>,
    ) -> Chosen {
        let origin = TrialOrigin::new(stage, key);
        let trial = self
            .trial_until(
                HeaderKind::Phrase(kind),
                catalog,
                deadline,
                &origin,
                stop,
                context,
            )
            .await;
        kanade_log::log!(
            info,
            "reminder_header_phrase",
            stage = stage.as_str(),
            kind = kind.as_str(),
            source = trial.source().as_str(),
            reason = trial.verdict.reason(),
            detail = trial.code(),
            estimates = context.is_some(),
        );
        Chosen {
            source: trial.source(),
            code: trial.code(),
            line: trial.line,
            slots: trial.slots,
        }
    }

    /// Logs a send that found no stored line and stores the seed.
    pub fn seed_at_send(&self, kind: Option<PhraseKind>) {
        let reason = if self.rewriter.is_none() {
            "no_rewriter"
        } else if self.persona.is_none() {
            "no_persona"
        } else {
            "not_ready"
        };
        match kind {
            None => kanade_log::log!(
                info,
                "day_of_heading",
                stage = "send",
                source = "seed",
                reason
            ),
            Some(kind) => kanade_log::log!(
                info,
                "reminder_header_phrase",
                stage = "send",
                source = "seed",
                reason,
                kind = kind.as_str(),
            ),
        }
    }

    /// One rewrite of `kind`'s seed through its gate, never slower than
    /// `deadline`. Stores nothing; writes one Rewrites-log row for `origin`
    /// when a log is attached. Sends no estimate context, so its line never
    /// holds a placeholder.
    pub async fn trial(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        origin: &TrialOrigin,
    ) -> Trial {
        self.trial_until(kind, catalog, deadline, origin, None, None)
            .await
    }

    /// As [`Self::trial`] with estimate `context` (the live header probes).
    pub async fn trial_with(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        origin: &TrialOrigin,
        context: Option<&HeaderContext>,
    ) -> Trial {
        self.trial_until(kind, catalog, deadline, origin, None, context)
            .await
    }

    /// As [`Self::trial_with`], ended by `stop` (the pre-generation
    /// worker's): a call in flight when it fires fails `Unavailable` with
    /// code `shutdown` and still writes its row; set before the call, the
    /// model is not called and nothing is logged.
    async fn trial_until(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        origin: &TrialOrigin,
        stop: Option<&watch::Receiver<bool>>,
        context: Option<&HeaderContext>,
    ) -> Trial {
        if stop.is_some_and(|stop| *stop.borrow()) {
            return shutdown_trial(kind);
        }
        // Digest phrases never carry estimates (D1).
        let context = context.filter(|_| kind != HeaderKind::Phrase(PhraseKind::Digest));
        let started = tokio::time::Instant::now();
        let (trial, prompt) = self.attempt(kind, catalog, deadline, stop, context).await;
        if let Some(log) = &self.log {
            log.record(RewriteAttempt {
                kind: kind.log_kind(),
                stage: origin.stage,
                context: origin.context.clone(),
                seed: seed_of(kind).to_owned(),
                verdict: trial.verdict.reason(),
                rule: trial.verdict.rule(),
                latency: prompt.is_some().then(|| started.elapsed()),
                line: Some(trial.line.clone()),
                prompt,
                detail: RewriteDetail {
                    reply: trial.output.clone().or_else(|| trial.detail.reply.clone()),
                    ..trial.detail.clone()
                },
            })
            .await;
        }
        trial
    }

    /// The trial and, when the model was called, the prompt it was given.
    async fn attempt(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        stop: Option<&watch::Receiver<bool>>,
        context: Option<&HeaderContext>,
    ) -> (Trial, Option<RewritePrompt>) {
        let seed = seed_of(kind);
        let fallback = |output, verdict, detail| Trial {
            output,
            line: seed.to_owned(),
            verdict,
            detail,
            slots: None,
        };
        let Some(rewriter) = &self.rewriter else {
            return (
                fallback(None, Verdict::NoRewriter, RewriteDetail::default()),
                None,
            );
        };
        let Some(persona) = self.persona.as_ref().and_then(|source| source()) else {
            return (
                fallback(None, Verdict::NoPersona, RewriteDetail::default()),
                None,
            );
        };
        let mood = context.map_or(NudgeMood::Playful, |context| context.mood);
        let prompt =
            RewritePrompt::header(&persona, mood, seed, context.map(|context| &context.facts));
        let call = tokio::time::timeout(deadline, rewriter.rewrite_detailed(&prompt, deadline));
        // `None`: the worker's stop fired while the call was in flight.
        let outcome = match stop {
            None => Some(call.await),
            Some(stop) => {
                let mut stop = stop.clone();
                tokio::select! {
                    biased;
                    outcome = call => Some(outcome),
                    Ok(_) = stop.wait_for(|stop| *stop) => None,
                }
            }
        };
        let sent = Some(prompt);
        let outcome = match outcome {
            None => return (shutdown_trial(kind), sent),
            Some(Err(_)) => {
                return (
                    fallback(None, Verdict::Timeout, RewriteDetail::default()),
                    sent,
                );
            }
            Some(Ok(outcome)) => outcome,
        };
        let text = match outcome.result {
            Err(failure) => {
                return (
                    fallback(None, Verdict::Failed(failure), outcome.detail),
                    sent,
                );
            }
            Ok(text) => text,
        };
        let words = self.words();
        let accepted = match (kind, context) {
            (_, Some(context)) => accept_header_with(&text, kind, context, catalog, &words)
                .map(|line| (line.template, Some(context.slots_for(&line.used)))),
            (HeaderKind::DayOf, None) => accept_rewrite_with(&text, seed, &words)
                .map(|line| (line, None))
                .map_err(Verdict::Rejected),
            (HeaderKind::Phrase(_), None) => accept_phrase_with(&text, seed, catalog, &words)
                .map(|line| (line, None))
                .map_err(Verdict::PhraseRejected),
        };
        let trial = match accepted {
            Ok((line, slots)) => Trial {
                output: Some(text),
                line,
                verdict: Verdict::Accepted,
                detail: outcome.detail,
                slots,
            },
            Err(verdict) => fallback(Some(text), verdict, outcome.detail),
        };
        (trial, sent)
    }
}

fn seed_of(kind: HeaderKind) -> &'static str {
    match kind {
        HeaderKind::DayOf => DAY_OF_HEADING_SEED,
        HeaderKind::Phrase(phrase) => phrase.seed(),
    }
}

/// A trial ended by the worker's stop: the seed, as when serve's shutdown
/// cutoff cuts the call.
fn shutdown_trial(kind: HeaderKind) -> Trial {
    Trial {
        output: None,
        line: seed_of(kind).to_owned(),
        verdict: Verdict::Failed(RewriteFailure::Unavailable),
        detail: RewriteDetail {
            code: Some("shutdown"),
            ..RewriteDetail::default()
        },
        slots: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kanade_domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};

    fn catalog() -> BossTable {
        BossTable::from_spec(&CatalogSpec {
            difficulties: vec![DifficultySpec {
                prefix: "x".into(),
                label: "Extreme".into(),
            }],
            bosses: vec![BossSpec {
                short: "Kalos".into(),
                full: Some("Gatekeeper Kalos".into()),
                aliases: vec!["The Gatekeeper".into()],
                difficulties: Some(vec!["x".into()]),
                ..BossSpec::default()
            }],
        })
        .unwrap()
    }

    #[test]
    fn each_failure_logs_its_own_reason() {
        assert_eq!(failure_reason(RewriteFailure::Unavailable), "unavailable");
        assert_eq!(failure_reason(RewriteFailure::Refused), "refused");
        assert_eq!(
            failure_reason(RewriteFailure::Misconfigured),
            "misconfigured"
        );
    }

    #[test]
    fn phrase_gate_accepts_persona_phrases_and_refuses_facts() {
        let catalog = catalog();
        for phrase in [
            "Onward, Papa~ Let’s charge!",
            "Kyahho~ Kirarin V!",
            "Mou... fine, go clear it!",
            "Hmph — fine, onward 🎉",
            "Eh… fine–let's roll ✨",
            "Waku waku!",
            "Let's go!",
            "Onward!",
        ] {
            assert_eq!(
                accept_phrase(phrase, COUNTDOWN_PHRASE_SEED, Some(&catalog)),
                Ok(phrase.to_owned()),
                "{phrase}"
            );
        }
        let long = format!("Wa{}h!", "a".repeat(MAX_PHRASE_CHARS));
        for (line, refusal) in [
            ("20:14", PhraseRejection::FactualTerm),
            ("Thu 10 Sep", PhraseRejection::FactualTerm),
            ("Thu, Sep!", PhraseRejection::FactualTerm),
            ("Confirmed!", PhraseRejection::FactualTerm),
            ("yes!", PhraseRejection::FactualTerm),
            ("Two runs are ready", PhraseRejection::FactualTerm),
            ("At quarter past eight", PhraseRejection::FactualTerm),
            ("Eight o'clock, team~", PhraseRejection::FactualTerm),
            ("Half past nine!", PhraseRejection::FactualTerm),
            ("Seven tonight!", PhraseRejection::FactualTerm),
            ("Twenty to ten, go!", PhraseRejection::FactualTerm),
            ("XKalos!", PhraseRejection::CatalogTerm),
            ("Gatekeeper Kalos", PhraseRejection::CatalogTerm),
            ("Go, Extreme crew~", PhraseRejection::CatalogTerm),
            (
                "https://example.test",
                PhraseRejection::UnsafeLine(Rejection::LineRules),
            ),
            (
                "Go <@1001>!",
                PhraseRejection::UnsafeLine(Rejection::LineRules),
            ),
            (
                "Onward!\nGo",
                PhraseRejection::UnsafeLine(Rejection::LineRules),
            ),
            ("**wow**", PhraseRejection::UnsafeLine(Rejection::Markup)),
            ("🎉✨", PhraseRejection::NoWords),
            ("Go go go go, go go go go go!", PhraseRejection::TooLong),
            (long.as_str(), PhraseRejection::TooLong),
        ] {
            assert_eq!(
                accept_phrase(line, COUNTDOWN_PHRASE_SEED, Some(&catalog)),
                Err(refusal),
                "{line}"
            );
        }
    }

    fn carrier(ign: &str, crit: bool, effective: &[f64]) -> kanade_domain::chardata::Carrier {
        kanade_domain::chardata::Carrier {
            ign: ign.into(),
            crit_assumed: crit,
            effective: effective.iter().copied().map(Some).collect(),
            per_boss: Vec::new(),
        }
    }

    /// An invented party: Lumirae carries most, then Tobiwan, then Kestrel;
    /// Pellow brings no character.
    fn estimate(crit: bool) -> crate::cards::estimates::RunEstimate {
        use kanade_chardata::scouter::CarrierOwner;
        let owner = |member: &str, character: &str| CarrierOwner {
            member_id: member.into(),
            character_id: character.into(),
        };
        crate::cards::estimates::RunEstimate {
            party: kanade_domain::chardata::PartyShares {
                bosses: vec!["HKalos".into(), "HLimbo".into()],
                carriers: vec![
                    carrier("Kestrel", false, &[10.0, 8.0]),
                    carrier("Lumirae", crit, &[60.0, 50.0]),
                    carrier("Tobiwan", false, &[30.0, 22.0]),
                ],
                missing: 0,
            },
            minutes: vec![Some(24.5), None],
            possible: vec![Some(true), None],
            owners: vec![owner("3", "k-1"), owner("1", "l-1"), owner("2", "t-1")],
            igns: vec![
                "Kestrel".into(),
                "Lumirae".into(),
                "LumiAlt".into(),
                "Tobiwan".into(),
            ],
            ..crate::cards::estimates::RunEstimate::default()
        }
    }

    fn context() -> HeaderContext {
        tease_context(&[])
    }

    /// [`context`] with `teasable` party members (teasing on).
    fn tease_context(teasable: &[Teasable]) -> HeaderContext {
        HeaderContext::build(
            &[estimate(false)],
            NudgeMood::Playful,
            ["Mika Sora".to_owned(), "pellow".to_owned(), "Q".to_owned()],
            teasable,
        )
        .expect("a fresh estimate gives context")
    }

    fn teasable(member: &str, note: Option<&str>) -> Teasable {
        Teasable {
            member_id: member.into(),
            note: note.map(str::to_owned),
        }
    }

    #[test]
    fn context_offers_the_top_two_carriers_and_never_their_names() {
        let context = context();
        let fills: Vec<(&str, &str, &str)> = context
            .fills
            .iter()
            .map(|fill| {
                (
                    fill.placeholder.as_str(),
                    fill.member_id.as_str(),
                    fill.ign.as_str(),
                )
            })
            .collect();
        assert_eq!(fills, [("{c1}", "1", "Lumirae"), ("{c2}", "2", "Tobiwan")]);
        assert_eq!(
            context
                .facts
                .slots
                .iter()
                .map(|slot| slot.share)
                .collect::<Vec<_>>(),
            [61, 29]
        );
        assert_eq!(context.facts.bosses.len(), 2);
        assert!(context.facts.bosses[0].complete && !context.facts.bosses[0].crit_assumed);
        let block = context.facts.block();
        for name in ["Lumirae", "Tobiwan", "Kestrel", "LumiAlt", "Mika", "pellow"] {
            assert!(!block.contains(name), "{name} reached the prompt");
        }
        assert!(context.names.iter().any(|name| name == "LumiAlt"));
        assert!(
            !context.names.iter().any(|name| name == "Q"),
            "too short to refuse"
        );
        // Gentle: totals only, no call-outs (D7).
        let gentle = HeaderContext::build(&[estimate(true)], NudgeMood::Gentle, [], &[]).unwrap();
        assert!(gentle.fills.is_empty() && gentle.facts.slots.is_empty());
        assert!(gentle.facts.bosses[0].crit_assumed);
        // Stale or missing: no context, so today's prompt (D2).
        let stale = crate::cards::estimates::RunEstimate {
            older_than: Some(7),
            ..estimate(false)
        };
        assert!(HeaderContext::build(&[stale], NudgeMood::Playful, [], &[]).is_none());
        assert!(HeaderContext::build(&[], NudgeMood::Playful, [], &[]).is_none());
        let nobody = crate::cards::estimates::RunEstimate {
            party: kanade_domain::chardata::PartyShares {
                bosses: vec!["HKalos".into()],
                carriers: Vec::new(),
                missing: 2,
            },
            ..crate::cards::estimates::RunEstimate::default()
        };
        assert!(HeaderContext::build(&[nobody], NudgeMood::Playful, [], &[]).is_none());
    }

    fn header(output: &str, kind: HeaderKind) -> Result<HeaderLine, Verdict> {
        accept_header_with(
            output,
            kind,
            &context(),
            Some(&catalog()),
            WordFilter::builtin(),
        )
    }

    #[test]
    fn the_header_gate_checks_slots_names_digits_and_the_filled_length() {
        let countdown = HeaderKind::Phrase(PhraseKind::Countdown);
        let accepted = header("Go {c1}, carry us home~", countdown).expect("accepted");
        assert_eq!(accepted.template, "Go {c1}, carry us home~");
        assert_eq!(accepted.used, ["{c1}"]);
        let slots = context().slots_for(&accepted.used);
        assert_eq!(
            fill_slots(&slots, &accepted.template).as_deref(),
            Some("Go Lumirae, carry us home~")
        );
        assert_eq!(slots.slots[0].character_id, "l-1");
        assert!(slots.used_estimates && !slots.slots[0].tease);
        let day_of = header("{c2} and {c1} lead the way — {day}", HeaderKind::DayOf).unwrap();
        assert_eq!(day_of.used, ["{c2}", "{c1}"]);
        // No slot at all is fine too.
        assert!(header("Onward, team~", countdown).is_ok());

        let long_tail = "a".repeat(52);
        let too_long = format!("{{c1}} {long_tail}!");
        for (line, kind, rule) in [
            ("Go {c3}!", countdown, SlotRule::UnknownSlot),
            ("{c1}, oh {c1}!", countdown, SlotRule::DuplicateSlot),
            ("Go Lumirae, go!", countdown, SlotRule::RawName),
            (
                "lumialt carries — {day}",
                HeaderKind::DayOf,
                SlotRule::RawName,
            ),
            ("Go mika sora!", countdown, SlotRule::RawName),
            (
                "{c1} hits 9000 — {day}",
                HeaderKind::DayOf,
                SlotRule::Digits,
            ),
            (too_long.as_str(), countdown, SlotRule::TooLongFilled),
        ] {
            assert_eq!(
                header(line, kind).map(|line| line.template),
                Err(Verdict::SlotRejected(rule)),
                "{line}"
            );
        }
        assert_eq!(
            Verdict::SlotRejected(SlotRule::TooLongFilled).reason(),
            "rejected"
        );
        for rule in [
            SlotRule::UnknownSlot,
            SlotRule::DuplicateSlot,
            SlotRule::TooManySlots,
            SlotRule::RawName,
            SlotRule::Digits,
            SlotRule::TooLongFilled,
            SlotRule::UnsafeFill,
        ] {
            assert!(rule.rule().len() <= 64);
        }
        // The phrase and day-of rules still apply to the stripped line.
        assert_eq!(
            header("{c1} beats Kalos!", countdown),
            Err(Verdict::PhraseRejected(PhraseRejection::CatalogTerm))
        );
        assert_eq!(
            header("{c1} rocks", HeaderKind::DayOf),
            Err(Verdict::Rejected(Rejection::Placeholders))
        );
    }

    /// Persona pools and the context-free gates keep refusing `{c1}`.
    #[test]
    fn persona_pools_still_refuse_slots() {
        assert!(check_nudge_line("Go {c1}!").is_err());
        assert_eq!(
            accept_phrase("Go {c1}!", COUNTDOWN_PHRASE_SEED, None),
            Err(PhraseRejection::UnsafeLine(Rejection::LineRules))
        );
        assert_eq!(
            accept_rewrite_with("{c1} — {day}", DAY_OF_HEADING_SEED, WordFilter::builtin()),
            Err(Rejection::LineRules)
        );
    }

    #[test]
    fn a_stored_map_fills_only_safe_names_and_leaves_no_brace() {
        let mut slots = context().slots_for(&["{c1}".to_owned()]);
        assert_eq!(
            fill_slots(&slots, "Go {c1}!").as_deref(),
            Some("Go Lumirae!")
        );
        assert_eq!(fill_slots(&slots, "Go {c1} and {c2}!"), None);
        slots.slots[0].ign = "<@1>".into();
        assert_eq!(fill_slots(&slots, "Go {c1}!"), None);
    }

    /// D11: the lowest share among opted-in party members with an estimate
    /// who hold no carrier slot gets the next `{cN}`, with their note; the
    /// facts are otherwise the ones without teasing.
    #[test]
    fn the_tease_slot_goes_to_the_lowest_opted_in_member() {
        let plain = context();
        // Tobiwan (2) is already a carrier; Kestrel (3) is the lowest.
        let teased = tease_context(&[
            teasable("2", None),
            teasable("3", Some("Always lost on the way to the portal")),
        ]);
        let fill = teased.fills.last().expect("a tease fill");
        assert_eq!(
            (
                fill.placeholder.as_str(),
                fill.member_id.as_str(),
                fill.ign.as_str(),
                fill.tease
            ),
            ("{c3}", "3", "Kestrel", true)
        );
        assert_eq!(teased.offered(), ["{c1}", "{c2}", "{c3}"]);
        let tease = teased.facts.tease.clone().expect("a tease slot");
        assert_eq!(tease.placeholder, "{c3}");
        assert!(tease.share < teased.facts.slots[1].share);
        assert_eq!(
            tease.note.as_deref(),
            Some("Always lost on the way to the portal")
        );
        assert_eq!(teased.facts.slots, plain.facts.slots);
        assert_eq!(teased.facts.bosses, plain.facts.bosses);
        assert!(!teased.facts.block().contains("Kestrel"));
        // Nobody else qualifies: a carrier only, a member with no estimate.
        for who in [vec![teasable("1", None)], vec![teasable("9", None)], vec![]] {
            let context = tease_context(&who);
            assert_eq!(context, plain);
            assert_eq!(context.facts.block(), plain.facts.block());
        }
        // Never in the gentle mood (D11).
        let gentle = HeaderContext::build(
            &[estimate(false)],
            NudgeMood::Gentle,
            [],
            &[teasable("3", None)],
        )
        .unwrap();
        assert!(gentle.fills.is_empty() && gentle.facts.tease.is_none());
    }

    /// A tease slot counts against the two-slot cap (D5) and is stored as
    /// a tease slot.
    #[test]
    fn a_tease_slot_counts_against_the_slot_rules() {
        let context = tease_context(&[teasable("3", None)]);
        let countdown = HeaderKind::Phrase(PhraseKind::Countdown);
        let gate =
            |line: &str| accept_header_with(line, countdown, &context, None, WordFilter::builtin());
        assert_eq!(
            gate("{c1}, {c2} and sleepy {c3}!").map(|line| line.template),
            Err(Verdict::SlotRejected(SlotRule::TooManySlots))
        );
        assert_eq!(
            gate("{c3}, oh {c3}!").map(|line| line.template),
            Err(Verdict::SlotRejected(SlotRule::DuplicateSlot))
        );
        let accepted = gate("{c1} carries, {c3} naps~").expect("two slots");
        let slots = context.slots_for(&accepted.used);
        assert_eq!(
            slots
                .slots
                .iter()
                .map(|slot| (slot.slot.as_str(), slot.tease))
                .collect::<Vec<_>>(),
            [("{c1}", false), ("{c3}", true)]
        );
        assert_eq!(
            fill_slots(&slots, &accepted.template).as_deref(),
            Some("Lumirae carries, Kestrel naps~")
        );
    }

    /// D10 at prompt build: an opted-out member is no candidate, and a note
    /// that now names someone on the roster (or can't be checked) is left
    /// out while the opt-in still counts.
    #[test]
    fn a_note_is_rechecked_against_the_roster_now() {
        use kanade_domain::members::{Member, MemberProfile};
        let profile = |tease_ok: bool, note: &str| MemberProfile {
            member: Member {
                user_id: "3".into(),
                ..Member::default()
            },
            tease_ok,
            running_joke: Some(note.into()),
            ..MemberProfile::default()
        };
        let roster: &[String] = &["Aria".to_owned(), "Kestrel".to_owned()];
        assert_eq!(
            Teasable::of(&profile(false, "Naps a lot"), Some(roster)),
            None
        );
        assert_eq!(
            Teasable::of(&profile(true, "Naps a lot"), Some(roster)),
            Some(teasable("3", Some("Naps a lot")))
        );
        for (note, names) in [
            ("Naps more than aria", Some(roster)),
            ("Naps a lot", None),
            ("Naps in #general", Some(roster)),
        ] {
            assert_eq!(
                Teasable::of(&profile(true, note), names),
                Some(teasable("3", None)),
                "{note}"
            );
        }
    }
}
