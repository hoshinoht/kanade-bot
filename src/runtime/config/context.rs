//! The `[models.context]` seed (`KANADE_MODEL_CONTEXT`): windows above the
//! hard cap are clamped (and reported so serve can log it); anything else
//! invalid refuses startup, a reserve that leaves no prompt room in the
//! call token budget included.

use crate::domain::settings::{ContextRole, ContextSettings, MAX_CONTEXT_TOKENS};
use crate::infrastructure::llm::{CALL_TOKEN_BUDGET, PROMPT_FLOOR_TOKENS, RESERVE_LIMIT};

use super::Error;

pub(super) const KEY: &str = "KANADE_MODEL_CONTEXT";

/// A parsed seed and the paths of the windows clamped to the hard cap.
pub(super) fn parse(value: &str) -> Result<(ContextSettings, Vec<String>), Error> {
    let mut context: ContextSettings = serde_json::from_str(value).map_err(|error| {
        Error::Configuration(format!("{KEY} is not valid context settings: {error}"))
    })?;
    let mut clamped = Vec::new();
    let mut window = |path: String, value: &mut u32| -> Result<(), Error> {
        if *value == 0 {
            return Err(Error::Configuration(format!(
                "models.context.{path} must be a positive token count"
            )));
        }
        if *value > MAX_CONTEXT_TOKENS {
            *value = MAX_CONTEXT_TOKENS;
            clamped.push(path);
        }
        Ok(())
    };
    window("cloud_default".into(), &mut context.cloud_default)?;
    window("local_default".into(), &mut context.local_default)?;
    for (name, role) in roles(&mut context) {
        if let Some(cap) = role.cap.as_mut() {
            window(format!("{name}.cap"), cap)?;
        }
    }
    for (alias, value) in &mut context.overrides {
        if alias.trim().is_empty() {
            return Err(Error::Configuration(
                "models.context.overrides needs model aliases as keys".into(),
            ));
        }
        window(format!("overrides.{alias}"), value)?;
    }
    // The seed's own fallback windows: a role without a published window
    // runs on a zone default (or less, under its cap).
    let floor = context.cloud_default.min(context.local_default);
    for (name, role) in roles(&mut context) {
        if role.reserve == 0 {
            return Err(Error::Configuration(format!(
                "models.context.{name}.reserve must be a positive token count"
            )));
        }
        if role.reserve > MAX_CONTEXT_TOKENS {
            return Err(Error::Configuration(format!(
                "models.context.{name}.reserve must be at most {MAX_CONTEXT_TOKENS}"
            )));
        }
        if role.reserve >= RESERVE_LIMIT {
            return Err(Error::Configuration(format!(
                "models.context.{name}.reserve ({}) must be below {RESERVE_LIMIT}: a call's token budget is {CALL_TOKEN_BUDGET} and {PROMPT_FLOOR_TOKENS} of it stays for the prompt",
                role.reserve
            )));
        }
        let window = role.cap.map_or(floor, |cap| cap.min(floor));
        if role.reserve >= window {
            return Err(Error::Configuration(format!(
                "models.context.{name}.reserve ({}) must be smaller than its window ({window})",
                role.reserve
            )));
        }
    }
    Ok((context, clamped))
}

fn roles(context: &mut ContextSettings) -> [(&'static str, &mut ContextRole); 3] {
    [
        ("chat", &mut context.chat),
        ("extraction", &mut context.extraction),
        ("rewrite", &mut context.rewrite),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(patch: serde_json::Value) -> serde_json::Value {
        let mut value = serde_json::to_value(ContextSettings::default()).unwrap();
        for (key, field) in patch.as_object().unwrap() {
            value[key] = field.clone();
        }
        value
    }

    fn parsed(patch: serde_json::Value) -> Result<(ContextSettings, Vec<String>), String> {
        parse(&seed(patch).to_string()).map_err(|error| error.to_string())
    }

    #[test]
    fn windows_above_the_hard_cap_clamp_and_are_reported() {
        let (context, clamped) = parsed(serde_json::json!({
            "cloud_default": 200_000,
            "chat": {"reserve": 1024, "cap": 131_073},
            "overrides": {"kanata/big": 1_000_000, "kanata/ok": 32_768},
        }))
        .unwrap();
        assert_eq!(context.cloud_default, MAX_CONTEXT_TOKENS);
        assert_eq!(context.chat.cap, Some(MAX_CONTEXT_TOKENS));
        assert_eq!(context.overrides["kanata/big"], MAX_CONTEXT_TOKENS);
        assert_eq!(context.overrides["kanata/ok"], 32_768);
        assert_eq!(
            clamped,
            ["cloud_default", "chat.cap", "overrides.kanata/big"]
        );
        let (_, none) = parsed(serde_json::json!({})).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn zero_windows_zero_reserves_and_oversized_reserves_refuse() {
        for (patch, message) in [
            (
                serde_json::json!({"local_default": 0}),
                "models.context.local_default must be a positive token count",
            ),
            (
                serde_json::json!({"overrides": {"kanata/x": 0}}),
                "models.context.overrides.kanata/x must be a positive token count",
            ),
            (
                serde_json::json!({"rewrite": {"reserve": 0}}),
                "models.context.rewrite.reserve must be a positive token count",
            ),
            (
                serde_json::json!({"rewrite": {"reserve": 131_073}}),
                "models.context.rewrite.reserve must be at most 131072",
            ),
            (
                serde_json::json!({"chat": {"reserve": 4096, "cap": 4096}}),
                "models.context.chat.reserve (4096) must be smaller than its window (4096)",
            ),
            (
                serde_json::json!({"extraction": {"reserve": 8192}}),
                "models.context.extraction.reserve (8192) must be smaller than its window (8192)",
            ),
            // Windows big enough, but no prompt fits the call token budget.
            (
                serde_json::json!({"local_default": 65_536, "rewrite": {"reserve": 16_000}}),
                "models.context.rewrite.reserve (16000) must be below 15360: a call's token budget is 16384 and 1024 of it stays for the prompt",
            ),
            (
                serde_json::json!({"local_default": 65_536, "chat": {"reserve": 15_360}}),
                "models.context.chat.reserve (15360) must be below 15360: a call's token budget is 16384 and 1024 of it stays for the prompt",
            ),
        ] {
            assert_eq!(parsed(patch.clone()).unwrap_err(), message, "{patch}");
        }
        let (fits, _) =
            parsed(serde_json::json!({"local_default": 65_536, "rewrite": {"reserve": 15_359}}))
                .unwrap();
        assert_eq!(fits.rewrite.reserve, 15_359, "just below the limit starts");
        let unknown = parse(r#"{"cloud_default": 1}"#).unwrap_err().to_string();
        assert!(unknown.starts_with("KANADE_MODEL_CONTEXT is not valid context settings"));
    }
}
