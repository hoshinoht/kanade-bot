use std::{
    collections::HashSet,
    fmt,
    ops::{Deref, DerefMut},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use super::super::codec::{DecodeError, IdentitySession, IssuedName, ScanNeedles};
use super::{LeakFound, ScanExemptions, finder::Finder};
use crate::infrastructure::llm::{ChatRequest, CompletionResponse};

type Shared = Arc<Mutex<Box<dyn IdentitySession>>>;

fn lock(shared: &Shared) -> MutexGuard<'_, Box<dyn IdentitySession>> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Provider-boundary scanner for one identity session. Governed sessions run
/// it on every request just before admission; it reads the session's needles
/// at that moment, so author labels registered mid-session are covered.
/// Inactive (passthrough) scanners check nothing.
#[derive(Clone, Default)]
pub struct LeakScanner {
    active: Option<Active>,
}

#[derive(Clone)]
struct Active {
    session: Shared,
    exemptions: Arc<ScanExemptions>,
    /// What the model itself wrote in this session (reply text and tool-call
    /// arguments, raw and trimmed). Sending its own words back discloses
    /// nothing new, so an assistant turn that repeats one exactly is not
    /// scanned (the model saying "will" must not refuse a member `Will`).
    echoes: Arc<Mutex<HashSet<String>>>,
}

impl LeakScanner {
    /// Scans nothing: passthrough sessions and callers without member data.
    pub fn off() -> Self {
        Self::default()
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// `Err` lists what kinds of raw identity `request` still carries; a
    /// session with no needles to report fails closed.
    pub fn scan(&self, request: &ChatRequest) -> Result<(), LeakFound> {
        let Some(active) = &self.active else {
            return Ok(());
        };
        let Some(needles) = lock(&active.session).scan_needles() else {
            return Err(LeakFound::unscannable());
        };
        let echoes = active.echoes.lock().unwrap_or_else(PoisonError::into_inner);
        let mut found = LeakFound::default();
        Finder::new(&needles, &active.exemptions).request(request, &echoes, &mut found);
        if found.is_empty() { Ok(()) } else { Err(found) }
    }

    /// Remember what the model answered, so repeating it later is not
    /// mistaken for a leak. Runs for every reply of a scanned session.
    pub fn echo(&self, response: &CompletionResponse) {
        let Some(active) = &self.active else {
            return;
        };
        let mut echoes = active.echoes.lock().unwrap_or_else(PoisonError::into_inner);
        let texts = response
            .content
            .iter()
            .chain(response.tool_calls.iter().map(|call| &call.arguments));
        for text in texts {
            echoes.insert(text.clone());
            echoes.insert(text.trim().to_owned());
        }
    }
}

impl fmt::Debug for LeakScanner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("LeakScanner");
        debug.field("active", &self.is_active());
        if let Some(active) = &self.active {
            debug.field("exemptions", &active.exemptions.len());
        }
        debug.finish()
    }
}

/// What [`super::super::open_session`] hands a port: the identity session
/// (derefs to it) and the scanner to attach to the governed model session.
pub struct IdentityGrant {
    session: Box<dyn IdentitySession>,
    scanner: LeakScanner,
}

impl IdentityGrant {
    pub(in crate::infrastructure::llm::identity) fn unscanned(
        session: Box<dyn IdentitySession>,
    ) -> Self {
        Self {
            session,
            scanner: LeakScanner::off(),
        }
    }

    /// The session is shared with the scanner, which reads its needles at
    /// scan time.
    pub(in crate::infrastructure::llm::identity) fn scanned(
        session: Box<dyn IdentitySession>,
        exemptions: Arc<ScanExemptions>,
    ) -> Self {
        let shared: Shared = Arc::new(Mutex::new(session));
        Self {
            session: Box::new(SharedSession(Arc::clone(&shared))),
            scanner: LeakScanner {
                active: Some(Active {
                    session: shared,
                    exemptions,
                    echoes: Arc::default(),
                }),
            },
        }
    }

    /// Attach with `Session::with_scanner` to every governed session whose
    /// requests carry this identity session's output.
    pub fn scanner(&self) -> LeakScanner {
        self.scanner.clone()
    }

    pub fn into_session(self) -> Box<dyn IdentitySession> {
        self.session
    }
}

impl Deref for IdentityGrant {
    type Target = Box<dyn IdentitySession>;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

impl DerefMut for IdentityGrant {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.session
    }
}

impl fmt::Debug for IdentityGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdentityGrant")
            .field("scanner", &self.scanner)
            .finish_non_exhaustive()
    }
}

/// The port's handle on a session the scanner also reads.
struct SharedSession(Shared);

impl IdentitySession for SharedSession {
    fn author_label(&mut self, user_id: &str, name: &str) -> String {
        lock(&self.0).author_label(user_id, name)
    }

    fn member_ref(&mut self, user_id: &str) -> String {
        lock(&self.0).member_ref(user_id)
    }

    fn mention(&mut self, user_id: &str) -> String {
        lock(&self.0).mention(user_id)
    }

    fn participants(&mut self, user_ids: &[&str]) -> Vec<String> {
        lock(&self.0).participants(user_ids)
    }

    fn text(&mut self, text: &str) -> String {
        lock(&self.0).text(text)
    }

    fn tool_result(&mut self, content: &str) -> String {
        lock(&self.0).tool_result(content)
    }

    fn participant_enum(&self) -> Option<Vec<String>> {
        lock(&self.0).participant_enum()
    }

    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        lock(&self.0).decode_ref(value)
    }

    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        lock(&self.0).decode_json(json)
    }

    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        lock(&self.0).decode_reply(text)
    }

    fn scan_needles(&self) -> Option<ScanNeedles> {
        lock(&self.0).scan_needles()
    }

    fn former_name(&mut self, user_id: &str, name: &str) {
        lock(&self.0).former_name(user_id, name);
    }

    fn masks(&self) -> bool {
        lock(&self.0).masks()
    }

    fn message_ref(&mut self, message_id: &str) -> String {
        lock(&self.0).message_ref(message_id)
    }

    fn decode_message_ref(&self, value: &str) -> Result<String, DecodeError> {
        lock(&self.0).decode_message_ref(value)
    }

    fn mapping(&self) -> Option<Vec<IssuedName>> {
        lock(&self.0).mapping()
    }
}
