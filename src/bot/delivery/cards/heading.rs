//! Persona-voiced reminder headers. Day-of keeps its original rewrite contract;
//! countdown and digest accept a short persona phrase that states no facts.
//! Rewrites run ahead of the send (`delivery::pregen`); a send never calls
//! the model and uses the stored line or the seed.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::watch;

use crate::chat::nudge::{
    NudgeRewriter, Rejection, RewriteAttempt, RewriteDetail, RewriteFailure, RewritePrompt,
    SharedRewriteSink, SharedRewriter, WordFilter, WordSource, accept_rewrite_with,
    failure_verdict,
};
use crate::chat::persona::{CompiledPersona, NudgeMood};
use crate::domain::catalog::BossTable;
use crate::domain::model_log::{RewriteKind, RewriteStage};
use crate::runtime::logging;

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
];

/// The guild default persona (bundle, no member profile), if chat has one.
pub type PersonaSource = Arc<dyn Fn() -> Option<CompiledPersona> + Send + Sync>;

/// A header rewritten ahead of its send: the line (the day filled in for
/// day-of), where it came from and the failure code behind a seed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub line: String,
    pub source: HeadingSource,
    pub code: Option<&'static str>,
}

/// Where a heading came from, for the `day_of_heading` log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeadingSource {
    Rewrite,
    Seed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhraseKind {
    Countdown,
    Digest,
}

impl PhraseKind {
    pub fn seed(self) -> &'static str {
        match self {
            Self::Countdown => COUNTDOWN_PHRASE_SEED,
            Self::Digest => DIGEST_PHRASE_SEED,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Countdown => "countdown",
            Self::Digest => "digest",
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

/// The outcome of one rewrite attempt, with the gate rule that refused it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    /// Day-of line rules.
    Rejected(Rejection),
    /// Countdown/digest phrase rules.
    PhraseRejected(PhraseRejection),
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
            Self::Rejected(_) | Self::PhraseRejected(_) => "rejected",
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
            _ => None,
        }
    }
}

/// One rewrite attempt: the model's raw reply (if any), the line to use
/// (the accepted rewrite or the seed; day-of keeps `{day}`), the verdict and
/// what the call reported (its error code above all).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trial {
    pub output: Option<String>,
    pub line: String,
    pub verdict: Verdict,
    pub detail: RewriteDetail,
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

impl HeadingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rewrite => "rewrite",
            Self::Seed => "seed",
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

    /// The prompt a rewrite would send, if one would be attempted.
    pub fn prompt(&self) -> Option<RewritePrompt> {
        self.rewriter.as_ref()?;
        let persona = (self.persona.as_ref()?)()?;
        Some(RewritePrompt::header(
            &persona,
            NudgeMood::Playful,
            DAY_OF_HEADING_SEED,
        ))
    }

    /// Whether a pre-generation pass has anything to call.
    pub fn enabled(&self) -> bool {
        self.rewriter.is_some() && self.persona.is_some()
    }

    /// The heading for `day` (e.g. `Fri 25 Sep`), rewritten ahead of the
    /// send for the card under `key` by `stage` (a batch or a catch-up);
    /// never slower than `deadline` and ended by the worker's `stop` (a cut
    /// call fails with code `shutdown`). Logs the source and the failure
    /// code, never the text.
    pub async fn choose(
        &self,
        day: &str,
        key: &str,
        stage: RewriteStage,
        deadline: Duration,
        stop: Option<&watch::Receiver<bool>>,
    ) -> Chosen {
        let origin = TrialOrigin::new(stage, key);
        let trial = self
            .trial_until(HeaderKind::DayOf, None, deadline, &origin, stop)
            .await;
        logging::event(
            "INFO",
            "day_of_heading",
            json!({
                "stage": stage.as_str(),
                "source": trial.source().as_str(),
                "reason": trial.verdict.reason(),
                "detail": trial.code(),
            }),
        );
        Chosen {
            line: trial.line.replace(DAY, day),
            source: trial.source(),
            code: trial.code(),
        }
    }

    /// A countdown or digest phrase rewritten ahead of the send for the card
    /// or digest under `key`. Its prompt contains only the persona and the
    /// code-owned seed.
    pub async fn choose_phrase(
        &self,
        kind: PhraseKind,
        catalog: Option<&BossTable>,
        key: &str,
        stage: RewriteStage,
        deadline: Duration,
        stop: Option<&watch::Receiver<bool>>,
    ) -> Chosen {
        let origin = TrialOrigin::new(stage, key);
        let trial = self
            .trial_until(HeaderKind::Phrase(kind), catalog, deadline, &origin, stop)
            .await;
        logging::event(
            "INFO",
            "reminder_header_phrase",
            json!({
                "stage": stage.as_str(),
                "kind": kind.as_str(),
                "source": trial.source().as_str(),
                "reason": trial.verdict.reason(),
                "detail": trial.code(),
            }),
        );
        Chosen {
            source: trial.source(),
            code: trial.code(),
            line: trial.line,
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
        let mut fields = json!({"stage": "send", "source": "seed", "reason": reason});
        let event = match kind {
            None => "day_of_heading",
            Some(kind) => {
                fields["kind"] = kind.as_str().into();
                "reminder_header_phrase"
            }
        };
        logging::event("INFO", event, fields);
    }

    /// One rewrite of `kind`'s seed through its gate, never slower than
    /// `deadline`. Stores nothing; writes one Rewrites-log row for `origin`
    /// when a log is attached.
    pub async fn trial(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        origin: &TrialOrigin,
    ) -> Trial {
        self.trial_until(kind, catalog, deadline, origin, None)
            .await
    }

    /// As [`Self::trial`], ended by `stop` (the pre-generation worker's): a
    /// call in flight when it fires fails `Unavailable` with code `shutdown`
    /// and still writes its row; set before the call, the model is not
    /// called and nothing is logged.
    async fn trial_until(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        origin: &TrialOrigin,
        stop: Option<&watch::Receiver<bool>>,
    ) -> Trial {
        if stop.is_some_and(|stop| *stop.borrow()) {
            return shutdown_trial(kind);
        }
        let started = tokio::time::Instant::now();
        let (trial, called) = self.attempt(kind, catalog, deadline, stop).await;
        if let Some(log) = &self.log {
            log.record(RewriteAttempt {
                kind: kind.log_kind(),
                stage: origin.stage,
                context: origin.context.clone(),
                seed: seed_of(kind).to_owned(),
                verdict: trial.verdict.reason(),
                rule: trial.verdict.rule(),
                latency: called.then(|| started.elapsed()),
                line: Some(trial.line.clone()),
                detail: RewriteDetail {
                    reply: trial.output.clone().or_else(|| trial.detail.reply.clone()),
                    ..trial.detail.clone()
                },
            })
            .await;
        }
        trial
    }

    /// The trial and whether the model was called.
    async fn attempt(
        &self,
        kind: HeaderKind,
        catalog: Option<&BossTable>,
        deadline: Duration,
        stop: Option<&watch::Receiver<bool>>,
    ) -> (Trial, bool) {
        let seed = seed_of(kind);
        let fallback = |output, verdict, detail| Trial {
            output,
            line: seed.to_owned(),
            verdict,
            detail,
        };
        let Some(rewriter) = &self.rewriter else {
            return (
                fallback(None, Verdict::NoRewriter, RewriteDetail::default()),
                false,
            );
        };
        let Some(persona) = self.persona.as_ref().and_then(|source| source()) else {
            return (
                fallback(None, Verdict::NoPersona, RewriteDetail::default()),
                false,
            );
        };
        let prompt = RewritePrompt::header(&persona, NudgeMood::Playful, seed);
        let call = tokio::time::timeout(deadline, rewriter.rewrite_detailed(&prompt, deadline));
        let outcome = match stop {
            None => call.await,
            Some(stop) => {
                let mut stop = stop.clone();
                tokio::select! {
                    biased;
                    outcome = call => outcome,
                    Ok(_) = stop.wait_for(|stop| *stop) => return (shutdown_trial(kind), true),
                }
            }
        };
        let outcome = match outcome {
            Err(_) => {
                return (
                    fallback(None, Verdict::Timeout, RewriteDetail::default()),
                    true,
                );
            }
            Ok(outcome) => outcome,
        };
        let text = match outcome.result {
            Err(failure) => {
                return (
                    fallback(None, Verdict::Failed(failure), outcome.detail),
                    true,
                );
            }
            Ok(text) => text,
        };
        let words = self.words();
        let accepted = match kind {
            HeaderKind::DayOf => {
                accept_rewrite_with(&text, seed, &words).map_err(Verdict::Rejected)
            }
            HeaderKind::Phrase(_) => {
                accept_phrase_with(&text, seed, catalog, &words).map_err(Verdict::PhraseRejected)
            }
        };
        let trial = match accepted {
            Ok(line) => Trial {
                output: Some(text),
                line,
                verdict: Verdict::Accepted,
                detail: outcome.detail,
            },
            Err(verdict) => fallback(Some(text), verdict, outcome.detail),
        };
        (trial, true)
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::catalog::{BossSpec, CatalogSpec, DifficultySpec};

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
}
