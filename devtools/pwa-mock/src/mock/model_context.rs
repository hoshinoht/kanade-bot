//! Context-window settings (`models.context`) and their per-role resolution,
//! mirroring the server's `resolve_context` and `validate_context` so the
//! admin app sees the same effective windows, 422s and notices.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const MAX_TOKENS: u32 = 131_072;
pub const LOCAL_WARNING_TOKENS: u32 = 16_384;
pub const LOCAL_WARNING: &str =
    "Context past 16k may result in degraded performance on local models.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub reserve: u32,
    #[serde(default)]
    pub cap: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub cloud_default: u32,
    pub local_default: u32,
    pub chat: Role,
    pub extraction: Role,
    pub rewrite: Role,
    #[serde(default)]
    pub overrides: BTreeMap<String, u32>,
}

impl Default for Settings {
    /// The server's defaults: current role reserves and a conservative local window.
    fn default() -> Self {
        Self {
            cloud_default: 65_536,
            local_default: 8_192,
            chat: Role {
                reserve: 1_024,
                cap: None,
            },
            extraction: Role {
                reserve: 2_500,
                cap: None,
            },
            rewrite: Role {
                reserve: 96,
                cap: None,
            },
            overrides: BTreeMap::new(),
        }
    }
}

/// What the catalog publishes for a listed route.
#[derive(Clone, Copy)]
pub struct Route {
    /// Listed and not leaving the homelab.
    pub local: bool,
    pub context_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

impl Settings {
    fn role(&self, role: &str) -> &Role {
        match role {
            "chat" => &self.chat,
            "extraction" => &self.extraction,
            _ => &self.rewrite,
        }
    }

    /// `roles.<role>.context`: the server's resolution order and clamps.
    pub fn resolve(&self, role: &str, alias: &str, route: Option<Route>) -> Value {
        let local = route.is_some_and(|r| r.local);
        let published = route.and_then(|r| r.context_tokens);
        let (mut window, source) = match self.overrides.get(alias) {
            Some(window) => (*window, "override"),
            None => match published {
                Some(window) => (window, "catalog"),
                None if local => (self.local_default, "local_default"),
                None => (self.cloud_default, "cloud_default"),
            },
        };
        let clamped_by_published = published.is_some_and(|p| window > p);
        if let Some(p) = published {
            window = window.min(p);
        }
        let clamped_by_hard_cap = window > MAX_TOKENS;
        window = window.min(MAX_TOKENS);
        let settings = self.role(role);
        let clamped_by_role_cap = settings.cap.is_some_and(|cap| window > cap);
        window = settings.cap.map_or(window, |cap| window.min(cap));
        let reserve = route
            .and_then(|r| r.max_output_tokens)
            .map_or(settings.reserve, |max| settings.reserve.min(max));
        json!({
            "window": window,
            "reserve": reserve,
            "prompt_budget": window.saturating_sub(reserve),
            "source": source,
            "clamped_by_published": clamped_by_published,
            "clamped_by_hard_cap": clamped_by_hard_cap,
            "clamped_by_role_cap": clamped_by_role_cap,
            "local_warning": local && window > LOCAL_WARNING_TOKENS,
        })
    }

    /// The server's save check, in its order and words. `Ok(true)` when a
    /// routed local role runs past the (non-blocking) warning threshold.
    pub fn validate(
        &self,
        roles: [(&str, &str); 3],
        route: impl Fn(&str) -> Option<Route>,
    ) -> Result<bool, String> {
        let valid = |value: u32| (1..=MAX_TOKENS).contains(&value);
        if !valid(self.cloud_default) || !valid(self.local_default) {
            return Err("Context defaults must be 1..=131072 tokens.".into());
        }
        for name in ["chat", "extraction", "rewrite"] {
            let role = self.role(name);
            if !valid(role.reserve) || role.cap.is_some_and(|cap| !valid(cap)) {
                return Err(format!(
                    "models.context.{name} reserve and cap must be 1..=131072 tokens."
                ));
            }
            if role.reserve >= RESERVE_LIMIT {
                return Err(format!(
                    "The {name} reserve ({}) must be below {RESERVE_LIMIT}: each call's token budget is 16384 and at least 1024 of it stays for the prompt.",
                    role.reserve
                ));
            }
        }
        for (alias, window) in &self.overrides {
            if alias.trim().is_empty() {
                return Err("models.context.overrides needs model aliases as keys.".into());
            }
            if !valid(*window) {
                return Err(format!(
                    "models.context.overrides.{alias} must be 1..=131072 tokens."
                ));
            }
            if let Some(maximum) = route(alias).and_then(|r| r.context_tokens)
                && *window > maximum
            {
                return Err(format!(
                    "models.context.overrides.{alias} exceeds Kanata's published context window ({maximum})."
                ));
            }
        }
        let mut warn = false;
        for (name, alias) in roles {
            if alias.is_empty() {
                continue;
            }
            let resolved = self.resolve(name, alias, route(alias));
            let window = resolved["window"].as_u64().unwrap_or(0);
            if resolved["reserve"].as_u64().unwrap_or(0) >= window {
                return Err(format!(
                    "models.context.{name}.reserve must be smaller than its effective window ({window})."
                ));
            }
            warn |= resolved["local_warning"] == true;
        }
        Ok(warn)
    }
}

/// A PATCH carries the complete object; anything else is the server's one refusal.
/// As the server's runner: a call's token budget less the prompt floor.
const RESERVE_LIMIT: u32 = 16_384 - 1_024;

pub fn parse(value: &Value) -> Result<Settings, String> {
    serde_json::from_value(value.clone())
        .map_err(|_| "models.context must be a complete context settings object.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL: Route = Route {
        local: true,
        context_tokens: None,
        max_output_tokens: Some(512),
    };

    #[test]
    fn a_reserve_without_prompt_room_in_the_call_budget_is_refused() {
        let mut s = Settings {
            local_default: 65_536,
            ..Settings::default()
        };
        s.rewrite.reserve = 16_000;
        let err = s
            .validate([("chat", ""), ("extraction", ""), ("rewrite", "")], |_| {
                None
            })
            .unwrap_err();
        assert!(
            err.starts_with("The rewrite reserve (16000) must be below 15360"),
            "{err}"
        );
        s.rewrite.reserve = 15_359;
        assert!(
            s.validate([("chat", ""), ("extraction", ""), ("rewrite", "")], |_| {
                None
            })
            .is_ok()
        );
    }

    #[test]
    fn resolution_follows_override_catalog_then_zone_default() {
        let mut s = Settings::default();
        let v = s.resolve("rewrite", "m", Some(LOCAL));
        assert_eq!(
            (v["window"].as_u64(), v["source"].as_str()),
            (Some(8_192), Some("local_default"))
        );
        let v = s.resolve("chat", "m", None);
        assert_eq!(v["source"], "cloud_default");
        s.overrides.insert("m".into(), 20_000);
        s.chat.reserve = 4_096;
        let v = s.resolve("chat", "m", Some(LOCAL));
        assert_eq!(
            (v["window"].as_u64(), v["reserve"].as_u64()),
            (Some(20_000), Some(512))
        );
        assert_eq!(v["local_warning"], true);
        s.chat.cap = Some(16_000);
        let v = s.resolve("chat", "m", Some(LOCAL));
        assert_eq!(
            (
                v["window"].as_u64(),
                v["clamped_by_role_cap"].as_bool(),
                v["local_warning"].as_bool()
            ),
            (Some(16_000), Some(true), Some(false))
        );
    }

    #[test]
    fn validation_refuses_like_the_server() {
        let published = |_: &str| {
            Some(Route {
                local: false,
                context_tokens: Some(32_768),
                max_output_tokens: None,
            })
        };
        let roles = [("extraction", "m"), ("chat", "m"), ("rewrite", "")];
        let mut s = Settings::default();
        assert_eq!(s.validate(roles, published), Ok(false));
        s.overrides.insert("m".into(), 40_000);
        assert!(
            s.validate(roles, published)
                .unwrap_err()
                .contains("published context window (32768)")
        );
        s.overrides.clear();
        s.chat.reserve = 32_768;
        assert!(
            s.validate(roles, published)
                .unwrap_err()
                .contains("must be below 15360")
        );
        (s.chat.reserve, s.chat.cap) = (8_192, Some(8_192));
        assert!(
            s.validate(roles, published)
                .unwrap_err()
                .contains("chat.reserve must be smaller")
        );
        s.chat.cap = None;
        s.chat.reserve = 131_073;
        assert!(
            s.validate(roles, published)
                .unwrap_err()
                .contains("1..=131072")
        );
        assert!(parse(&json!({ "cloud_default": 1 })).is_err());
        assert!(parse(&json!({ "cloud_default": -1, "local_default": 1, "chat": { "reserve": 1 }, "extraction": { "reserve": 1 }, "rewrite": { "reserve": 1 } })).is_err());
    }
}
