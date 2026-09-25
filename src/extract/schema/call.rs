//! v4 `Extractor.extract` as a pure retry decision: the caller sends
//! [`ExtractionAttempts::messages`], reports what happened, and either sends
//! again or has the final [`ExtractionCall`]. Nothing here performs I/O.
//!
//! With a governed extraction session the first attempt is `complete` and a
//! [`Next::Retry`] is `answer_retry`, both with `extraction_request` built from
//! the current [`ExtractionAttempts::messages`].

use std::time::Duration;

use super::{Extraction, parse_response};
use crate::extract::prompt::prompt_text;
use crate::infrastructure::llm::Message;
use crate::infrastructure::llm::identity::{DecodeError, IdentitySession};

/// Map the identity-bearing fields back: `participants` are member refs (v4's
/// coercion has already dropped any `<@!>` wrapping) and `summary` is text
/// shown to members. Other fields carry no identities.
fn decode(
    mut extraction: Extraction,
    session: &dyn IdentitySession,
) -> Result<Extraction, DecodeError> {
    for amendment in &mut extraction.amendments {
        for participant in &mut amendment.participants {
            *participant = session.decode_ref(participant)?;
        }
    }
    extraction.summary = session.decode_reply(&extraction.summary)?;
    Ok(extraction)
}

/// v4 `RETRY_INSTRUCTION`, sent after the first answer fails to validate.
pub fn retry_instruction(error: &str) -> String {
    format!(
        "Your previous answer did not fit the schema:\n{error}\nAnswer again with the same \
         information in the required shape. Do not add anything the messages do not say."
    )
}

/// What one attempt produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptOutcome {
    /// The model answered; `None` content is an empty answer.
    Reply {
        content: Option<String>,
        reasoning: Option<String>,
    },
    /// No answer within the configured limit.
    TimedOut { limit: Duration },
    /// The extraction model is not configured; reported once by the caller.
    Misconfigured { detail: String },
    /// Any other failure, already rendered as `Kind: message`.
    Failed { detail: String },
}

/// The outcome of one guarded extraction: always returned, never raised.
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractionCall {
    /// `Some` only when an answer validated; `None` is the quarantine.
    pub extraction: Option<Extraction>,
    pub error: Option<String>,
    pub attempts: u8,
    pub raw: String,
    pub thinking: String,
    pub misconfigured: bool,
    /// The initial messages joined by blank lines, for the extraction log.
    pub prompt: String,
}

impl ExtractionCall {
    pub fn ok(&self) -> bool {
        self.extraction.is_some()
    }
}

/// What to do after reporting an attempt.
#[derive(Clone, Debug, PartialEq)]
pub enum Next {
    /// Send [`ExtractionAttempts::messages`] again.
    Retry,
    Done(ExtractionCall),
}

/// At most two attempts: one retry carrying the validation error, after a bad
/// answer only. Transport failures, timeouts and misconfiguration end the call.
#[derive(Clone, Debug)]
pub struct ExtractionAttempts {
    conversation: Vec<Message>,
    prompt: String,
    attempt: u8,
    raw: String,
    thinking: String,
}

impl ExtractionAttempts {
    pub const MAX_ATTEMPTS: u8 = 2;

    pub fn new(messages: Vec<Message>) -> Self {
        Self {
            prompt: prompt_text(&messages),
            conversation: messages,
            attempt: 1,
            raw: String::new(),
            thinking: String::new(),
        }
    }

    /// The messages to send for the current attempt.
    pub fn messages(&self) -> &[Message] {
        &self.conversation
    }

    fn done(
        &self,
        extraction: Option<Extraction>,
        error: Option<String>,
        misconfigured: bool,
    ) -> Next {
        Next::Done(ExtractionCall {
            extraction,
            error,
            attempts: self.attempt,
            raw: self.raw.clone(),
            thinking: self.thinking.clone(),
            misconfigured,
            prompt: self.prompt.clone(),
        })
    }

    /// Report the current attempt. The validated reply is decoded through
    /// `session`; an unknown identity ref is a malformed answer.
    pub fn record(&mut self, outcome: AttemptOutcome, session: &dyn IdentitySession) -> Next {
        let (content, reasoning) = match outcome {
            AttemptOutcome::Reply { content, reasoning } => (content, reasoning),
            AttemptOutcome::TimedOut { limit } => {
                let error = format!(
                    "the model did not answer within {:.0}s",
                    limit.as_secs_f64()
                );
                return self.done(None, Some(error), false);
            }
            AttemptOutcome::Misconfigured { detail } => return self.done(None, Some(detail), true),
            AttemptOutcome::Failed { detail } => return self.done(None, Some(detail), false),
        };
        self.raw = crate::domain::pytext::strip(content.as_deref().unwrap_or_default()).to_owned();
        self.thinking =
            crate::domain::pytext::strip(reasoning.as_deref().unwrap_or_default()).to_owned();
        let parsed = parse_response(&self.raw)
            .map_err(|error| error.to_string())
            .and_then(|extraction| decode(extraction, session).map_err(|error| error.to_string()));
        let error = match parsed {
            Ok(extraction) => return self.done(Some(extraction), None, false),
            Err(error) => error,
        };
        if self.attempt >= Self::MAX_ATTEMPTS {
            return self.done(None, Some(error), false);
        }
        // The gateway rejects an empty assistant turn.
        if !self.raw.is_empty() {
            self.conversation.push(Message::Assistant {
                content: Some(self.raw.clone()),
                tool_calls: Vec::new(),
            });
        }
        self.conversation.push(Message::User {
            content: retry_instruction(&error),
        });
        self.attempt += 1;
        Next::Retry
    }
}
