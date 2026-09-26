use tokio::time::Instant;

use super::super::{
    ChatRequest, CompletionResponse,
    governor::{Attempt, CallKind, Outcome, Permit, Random, Refused},
    identity::{LeakFound, LeakScanner},
};

/// Admission for one runner call: every provider request passes the permit's
/// rate ceiling, breaker and (for retries) retry budget, and is capped by the
/// session's request count.
pub(in crate::infrastructure::llm) struct Gate<'a> {
    permit: Option<&'a Permit>,
    kind: CallKind,
    deadline: Option<Instant>,
    /// The session's own counter, bumped at admission so a cancelled call still counts.
    used: &'a mut u32,
    cap: u32,
    retry_next: bool,
    random: &'a dyn Random,
    attempt: Option<Attempt>,
    /// Session id; each request is tagged `{tag}-{n}` for gateway log correlation.
    tag: Option<&'a str>,
    scanner: Option<&'a LeakScanner>,
}

pub(in crate::infrastructure::llm) enum Denied {
    Governor(Refused),
    RequestLimit,
}

impl<'a> Gate<'a> {
    /// Admits while `*used < cap`; `retry_first` makes the first request spend
    /// retry budget (requeue, clean retry).
    pub(in crate::infrastructure::llm) fn governed(
        permit: &'a Permit,
        kind: CallKind,
        deadline: Instant,
        used: &'a mut u32,
        cap: u32,
        retry_first: bool,
        random: &'a dyn Random,
    ) -> Self {
        Self {
            permit: Some(permit),
            kind,
            deadline: Some(deadline),
            used,
            cap,
            retry_next: retry_first,
            random,
            attempt: None,
            tag: None,
            scanner: None,
        }
    }

    pub(in crate::infrastructure::llm) fn tagged(mut self, tag: &'a str) -> Self {
        self.tag = Some(tag);
        self
    }

    pub(in crate::infrastructure::llm) fn scanned(mut self, scanner: &'a LeakScanner) -> Self {
        self.scanner = Some(scanner);
        self
    }

    /// Boundary scan of the exact request about to be sent; runs before
    /// admission so a refusal spends no request, rate token or retry.
    pub(super) fn scan(&self, request: &ChatRequest) -> Result<(), LeakFound> {
        self.scanner.map_or(Ok(()), |scanner| scanner.scan(request))
    }

    /// Remembers a reply so the scanner lets the model's own words back in.
    pub(super) fn echo(&self, response: &CompletionResponse) {
        if let Some(scanner) = self.scanner {
            scanner.echo(response);
        }
    }

    /// Id of the request admitted last (numbered from 1 within the session).
    pub(super) fn request_id(&self) -> Option<String> {
        self.tag.map(|tag| format!("{tag}-{}", *self.used))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn ungoverned(random: &'a dyn Random, used: &'a mut u32, kind: CallKind) -> Self {
        Self {
            permit: None,
            kind,
            deadline: None,
            used,
            cap: u32::MAX,
            retry_next: false,
            random,
            attempt: None,
            tag: None,
            scanner: None,
        }
    }

    pub(super) fn kind(&self) -> CallKind {
        self.kind
    }

    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    pub(super) fn random(&self) -> &dyn Random {
        self.random
    }

    pub(super) fn has_room(&self) -> bool {
        *self.used < self.cap
    }

    /// Asked before backing off so a denied retry costs no sleep.
    pub(super) fn check_retry(&self) -> Result<(), Denied> {
        if !self.has_room() {
            return Err(Denied::RequestLimit);
        }
        match self.permit {
            Some(permit) => permit.check_retry(false).map_err(Denied::Governor),
            None => Ok(()),
        }
    }

    pub(super) async fn admit(&mut self, retry: bool, deadline: Instant) -> Result<(), Denied> {
        if !self.has_room() {
            return Err(Denied::RequestLimit);
        }
        let retry = retry || std::mem::take(&mut self.retry_next);
        if let Some(permit) = self.permit {
            let wait = deadline.saturating_duration_since(Instant::now());
            // Try-only kinds (rewrite, pre-screen) never wait for a rate token
            // and never retry.
            let attempt = match (self.kind.may_wait(), retry) {
                (false, false) => permit.try_begin_request(),
                (false, true) => Err(Refused::MustNotWait),
                (true, true) => permit.begin_retry(wait).await,
                (true, false) => permit.begin_request(wait).await,
            }
            .map_err(Denied::Governor)?;
            self.attempt = Some(attempt);
        }
        *self.used += 1;
        Ok(())
    }

    /// `None` abandons the attempt (breaker-neutral), e.g. when our own deadline cut it off.
    pub(super) fn finish(&mut self, outcome: Option<Outcome>) {
        if let (Some(attempt), Some(outcome)) = (self.attempt.take(), outcome) {
            attempt.finish(outcome);
        }
    }
}
