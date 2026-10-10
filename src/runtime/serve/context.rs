//! Live context windows for extraction and heading rewrites: each resolves
//! from the settings saved at the moment it is asked, over the cached catalog,
//! so a Config save applies to the next call without a restart.

use std::sync::Arc;

use tokio::sync::watch;

use crate::{
    api::admin::config::SettingsChanged,
    domain::settings::RuntimeSettings,
    infrastructure::llm::{
        governor::Role,
        setup::{ContextResolution, ModelStack, resolve_context},
    },
};

/// Resolves `role`'s context for an alias from the latest saved settings.
pub type Resolver = Arc<dyn Fn(&str) -> ContextResolution + Send + Sync>;

/// The config desk's changes, or `settings` fixed when nothing publishes.
pub fn changes_or(
    changes: Option<watch::Receiver<SettingsChanged>>,
    settings: &RuntimeSettings,
) -> watch::Receiver<SettingsChanged> {
    changes.unwrap_or_else(|| {
        watch::channel(SettingsChanged {
            revision: 0,
            section: None,
            actor: None,
            settings: Arc::new(settings.clone()),
        })
        .1
    })
}

pub fn resolver(
    stack: Arc<ModelStack>,
    settings: watch::Receiver<SettingsChanged>,
    role: Role,
) -> Resolver {
    Arc::new(move |alias| {
        let saved = Arc::clone(&settings.borrow().settings);
        resolve_context(&saved.models.context, &stack.catalog(), role, alias)
    })
}
