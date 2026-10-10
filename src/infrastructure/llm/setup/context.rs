//! Pure context-window resolution over saved settings and the cached catalog.

use crate::{
    domain::settings::{
        ContextRole, ContextSettings, LOCAL_CONTEXT_WARNING_TOKENS, MAX_CONTEXT_TOKENS,
    },
    infrastructure::llm::governor::Role,
};

use super::CatalogSnapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSource {
    Override,
    Catalog,
    CloudDefault,
    LocalDefault,
}

impl ContextSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Override => "override",
            Self::Catalog => "catalog",
            Self::CloudDefault => "cloud_default",
            Self::LocalDefault => "local_default",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextResolution {
    pub window: u32,
    /// The completion reserve requests ask for (`max_output_tokens`); the
    /// body carries it as `max_tokens` only with sampling controls.
    pub reserve: u32,
    pub prompt_budget: u32,
    pub source: ContextSource,
    pub clamped_by_published: bool,
    pub clamped_by_hard_cap: bool,
    pub clamped_by_role_cap: bool,
    pub local_warning: bool,
}

impl ContextResolution {
    /// Saved settings can still leave no prompt room at runtime (defaults, a
    /// seed, or a catalog that later lowers its published window).
    pub fn reserve_fills_window(&self) -> bool {
        self.reserve >= self.window
    }

    /// The operator warning for [`Self::reserve_fills_window`].
    pub fn reserve_warning(&self, role: Role, alias: &str) -> String {
        format!(
            "{} context reserve {} is not smaller than {alias}'s effective window {}; its prompts cannot fit",
            role.as_str(),
            self.reserve,
            self.window
        )
    }
}

fn role_settings(settings: &ContextSettings, role: Role) -> &ContextRole {
    match role {
        Role::Chat => &settings.chat,
        Role::Extraction => &settings.extraction,
        Role::Rewrite => &settings.rewrite,
    }
}

/// Resolves one role route. The caller validates `reserve < window` before
/// saving; the defensive `saturating_sub` keeps a malformed historical row
/// from underflowing while still reporting a zero prompt budget.
pub fn resolve_context(
    settings: &ContextSettings,
    catalog: &CatalogSnapshot,
    role: Role,
    alias: &str,
) -> ContextResolution {
    let model = catalog.models.iter().find(|model| model.alias == alias);
    let local = model.is_some_and(|model| !model.leaves_homelab);
    let (mut window, source) = match settings.overrides.get(alias).copied() {
        Some(window) => (window, ContextSource::Override),
        None => match model.and_then(|model| model.context_tokens) {
            Some(window) => (window, ContextSource::Catalog),
            None if local => (settings.local_default, ContextSource::LocalDefault),
            None => (settings.cloud_default, ContextSource::CloudDefault),
        },
    };
    let mut clamped_by_published = false;
    if let Some(published) = model.and_then(|model| model.context_tokens)
        && window > published
    {
        window = published;
        clamped_by_published = true;
    }
    let clamped_by_hard_cap = window > MAX_CONTEXT_TOKENS;
    window = window.min(MAX_CONTEXT_TOKENS);
    let role = role_settings(settings, role);
    let clamped_by_role_cap = role.cap.is_some_and(|cap| window > cap);
    window = role.cap.map_or(window, |cap| window.min(cap));
    let reserve = model
        .and_then(|model| model.max_output_tokens)
        .map_or(role.reserve, |maximum| role.reserve.min(maximum));
    ContextResolution {
        window,
        reserve,
        prompt_budget: window.saturating_sub(reserve),
        source,
        clamped_by_published,
        clamped_by_hard_cap,
        clamped_by_role_cap,
        local_warning: local && window > LOCAL_CONTEXT_WARNING_TOKENS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::llm::setup::CatalogModel;

    fn model(context: Option<u32>, output: Option<u32>, local: bool) -> CatalogModel {
        CatalogModel {
            alias: "m".into(),
            published: true,
            trust_zone: None,
            leaves_homelab: !local,
            reasoning_control: false,
            reasoning_efforts: None,
            structured_output: false,
            sampling_controls: false,
            function_tools: false,
            context_tokens: context,
            max_output_tokens: output,
            admission: None,
        }
    }

    #[test]
    fn precedence_caps_reserve_and_warning_are_resolved_together() {
        let mut settings = ContextSettings::default();
        settings.overrides.insert("m".into(), 200_000);
        settings.chat.cap = Some(60_000);
        settings.chat.reserve = 5_000;
        let catalog = CatalogSnapshot {
            listed: true,
            models: vec![model(Some(65_000), Some(4_000), true)],
        };
        let got = resolve_context(&settings, &catalog, Role::Chat, "m");
        assert_eq!(got.source, ContextSource::Override);
        assert_eq!(got.window, 60_000);
        assert!(got.clamped_by_published && got.clamped_by_role_cap);
        assert_eq!(got.reserve, 4_000);
        assert!(got.local_warning);
    }

    fn one(model: CatalogModel) -> CatalogSnapshot {
        CatalogSnapshot {
            listed: true,
            models: vec![model],
        }
    }

    #[test]
    fn each_source_and_clamp_applies_in_order() {
        let mut settings = ContextSettings::default();
        // Catalog beats the zone default; an unconstrained route keeps the reserve.
        let got = resolve_context(
            &settings,
            &one(model(Some(40_000), None, false)),
            Role::Extraction,
            "m",
        );
        assert_eq!(
            (got.source, got.window, got.reserve),
            (ContextSource::Catalog, 40_000, 2_500)
        );
        assert_eq!(got.prompt_budget, 37_500);
        assert!(!got.clamped_by_published && !got.clamped_by_hard_cap && !got.clamped_by_role_cap);
        // A published window past the hard cap.
        let got = resolve_context(
            &settings,
            &one(model(Some(1_000_000), None, false)),
            Role::Chat,
            "m",
        );
        assert_eq!(got.window, MAX_CONTEXT_TOKENS);
        assert!(got.clamped_by_hard_cap && !got.clamped_by_published);
        // An override below the published window wins; above it is clamped.
        settings.overrides.insert("m".into(), 20_000);
        let published = one(model(Some(40_000), Some(64), false));
        let got = resolve_context(&settings, &published, Role::Rewrite, "m");
        assert_eq!((got.source, got.window), (ContextSource::Override, 20_000));
        assert_eq!(got.reserve, 64, "the rewrite reserve clamps to max_output");
        settings.overrides.insert("m".into(), 50_000);
        let got = resolve_context(&settings, &published, Role::Rewrite, "m");
        assert_eq!(got.window, 40_000);
        assert!(got.clamped_by_published);
        // An unlisted alias is treated as cloud.
        let got = resolve_context(&settings, &published, Role::Chat, "unlisted");
        assert_eq!(
            (got.source, got.window, got.local_warning),
            (ContextSource::CloudDefault, 65_536, false)
        );
    }

    #[test]
    fn only_local_windows_past_16k_warn() {
        let settings = ContextSettings::default();
        let warns = |context, local| {
            resolve_context(
                &settings,
                &one(model(Some(context), None, local)),
                Role::Chat,
                "m",
            )
            .local_warning
        };
        assert!(!warns(16_384, true));
        assert!(warns(16_385, true));
        assert!(!warns(131_072, false));
    }

    #[test]
    fn zone_defaults_are_distinct() {
        let settings = ContextSettings::default();
        let local = CatalogSnapshot {
            listed: true,
            models: vec![model(None, None, true)],
        };
        let cloud = CatalogSnapshot {
            listed: true,
            models: vec![model(None, None, false)],
        };
        assert_eq!(
            resolve_context(&settings, &local, Role::Extraction, "m").window,
            8_192
        );
        assert_eq!(
            resolve_context(&settings, &cloud, Role::Extraction, "m").window,
            65_536
        );
    }
}
