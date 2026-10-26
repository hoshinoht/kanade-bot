//! Test support: one process-wide hook that sees every model request after
//! its connection opens and before any byte is sent, and may refuse it (the
//! V02 quality harness counts requests, caps them and checks their peers).

use std::{
    net::SocketAddr,
    sync::{Arc, PoisonError, RwLock},
};

/// What the hook sees of one request; never the body or the key.
#[derive(Clone, Copy, Debug)]
pub struct SentRequest<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub peer: Option<SocketAddr>,
}

/// `false` refuses the request unsent.
pub type RequestHook = Arc<dyn Fn(&SentRequest<'_>) -> bool + Send + Sync>;

static HOOK: RwLock<Option<RequestHook>> = RwLock::new(None);

/// Install (or with `None` remove) the hook for every provider in the process.
pub fn set_request_hook(hook: Option<RequestHook>) {
    *HOOK.write().unwrap_or_else(PoisonError::into_inner) = hook;
}

pub(super) fn admit(request: &SentRequest<'_>) -> bool {
    let hook = HOOK.read().unwrap_or_else(PoisonError::into_inner).clone();
    hook.is_none_or(|hook| hook(request))
}
