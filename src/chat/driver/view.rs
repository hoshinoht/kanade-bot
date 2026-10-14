//! A read handle on the running driver for health, `/limits` and the API,
//! created before the driver exists (the API is composed first).

use std::sync::{Arc, OnceLock};

use crate::chat::pilot::{Allowance, AllowanceSnapshot, LimitsView};

/// What readers of the running pilot see.
pub trait ChatView: Send + Sync {
    fn limits(&self) -> LimitsView;
    /// Clears the in-memory answer allowance for one member.
    fn reset_allowance(&self, member_id: &str);
    /// `disabled`, `idle`, `busy` or `degraded`.
    fn status(&self) -> &'static str;
}

/// Empty until serve starts the driver; then fixed for the process.
#[derive(Default)]
pub struct ChatHandle(OnceLock<Arc<dyn ChatView>>);

impl std::fmt::Debug for ChatHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChatHandle")
            .field("started", &self.0.get().is_some())
            .finish()
    }
}

impl ChatHandle {
    /// First call wins; later ones are ignored.
    pub fn set(&self, view: Arc<dyn ChatView>) {
        let _ = self.0.set(view);
    }

    /// The Limits page's view; `None` before the driver starts.
    pub fn limits(&self) -> Option<LimitsView> {
        self.0.get().map(|view| view.limits())
    }

    /// Allowance only (`/limits`); the defaults before the driver starts.
    pub fn allowance(&self) -> AllowanceSnapshot {
        self.limits()
            .map_or_else(|| Allowance::default().snapshot(0.0), |view| view.allowance)
    }

    /// Clears one live member window. There is no driver to change before
    /// chat starts, so callers can distinguish that unavailable case.
    pub fn reset_allowance(&self, member_id: &str) -> bool {
        let Some(view) = self.0.get() else {
            return false;
        };
        view.reset_allowance(member_id);
        true
    }

    pub fn status(&self) -> &'static str {
        self.0.get().map_or("disabled", |view| view.status())
    }
}
