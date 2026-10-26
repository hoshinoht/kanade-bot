//! `KANADE_MODEL_CAPABILITIES`: operator capability declarations per model
//! alias, as a JSON object of alias → fields (`kanade.toml`
//! `[models.capabilities."<alias>"]` arrives in this form). They are used only
//! where the gateway publishes no metadata for the alias (published metadata,
//! such as Kanata's, always wins), and they also list models a gateway does
//! not. A field left out keeps the minimal profile's value; an unknown field
//! or a value out of range refuses startup. Trust zones and admission limits
//! are not declarable: routes stay external until a listing publishes a home
//! zone, and capacity comes from permits or groups.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::{Error, models::is_alias, non_empty};
use kanade_llm::{Effort, MAX_OUTPUT_TOKENS, ModelCapabilities, OutputLimit};

pub(super) const KEY: &str = "KANADE_MODEL_CAPABILITIES";

const FIELDS: &str = "structured_output, function_tools, sampling_controls, reasoning_control, \
                      reasoning_efforts, context_tokens, max_output_tokens or output_limit";

fn invalid(rule: &str) -> Error {
    Error::Configuration(format!("{KEY} {rule}"))
}

pub(super) fn parse(
    values: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, ModelCapabilities>, Error> {
    let Some(text) = non_empty(values, KEY) else {
        return Ok(BTreeMap::new());
    };
    let Ok(Value::Object(aliases)) = serde_json::from_str(text) else {
        return Err(invalid(
            "must be a JSON object of model aliases to capability tables",
        ));
    };
    aliases
        .into_iter()
        .map(|(alias, fields)| {
            if !is_alias(&alias) {
                return Err(invalid(
                    "keys must be model aliases (printable ASCII, at most 200 bytes)",
                ));
            }
            let declared = declaration(&alias, &fields)?;
            Ok((alias, declared))
        })
        .collect()
}

fn declaration(alias: &str, fields: &Value) -> Result<ModelCapabilities, Error> {
    let Value::Object(fields) = fields else {
        return Err(invalid(&format!(
            "`{alias}` must be a table of capability fields"
        )));
    };
    let wrong =
        |field: &str, expected: &str| invalid(&format!("`{alias}`.{field} must be {expected}"));
    let mut declared = ModelCapabilities::minimal();
    for (field, value) in fields {
        let flag = || value.as_bool().ok_or_else(|| wrong(field, "true or false"));
        let tokens = |maximum: u32| {
            value
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .filter(|number| (1..=maximum).contains(number))
                .ok_or_else(|| wrong(field, &format!("a token count from 1 to {maximum}")))
        };
        match field.as_str() {
            "structured_output" => declared.structured_output = flag()?,
            "function_tools" => declared.function_tools = flag()?,
            "sampling_controls" => declared.sampling_controls = flag()?,
            "reasoning_control" => declared.reasoning_control = flag()?,
            "reasoning_efforts" => {
                declared.reasoning_efforts = Some(efforts(value).ok_or_else(|| {
                    wrong(
                        field,
                        "a non-empty list of distinct levels: off, minimal, low, medium, high, xhigh or max",
                    )
                })?);
            }
            "context_tokens" => declared.context_tokens = Some(tokens(u32::MAX)?),
            "max_output_tokens" => declared.max_output_tokens = Some(tokens(MAX_OUTPUT_TOKENS)?),
            // Sends the size under this name even without sampling controls.
            "output_limit" => {
                declared.output_limit = Some(match value.as_str() {
                    Some("max_tokens") => OutputLimit::MaxTokens,
                    Some("max_completion_tokens") => OutputLimit::MaxCompletionTokens,
                    _ => return Err(wrong(field, "\"max_tokens\" or \"max_completion_tokens\"")),
                });
            }
            _ => {
                return Err(invalid(&format!(
                    "`{alias}` has an unknown field `{field}`; declare {FIELDS}"
                )));
            }
        }
    }
    // Without reasoning control nothing is sent, so a level list would be ignored.
    if declared.reasoning_efforts.is_some() && !declared.reasoning_control {
        return Err(invalid(&format!(
            "`{alias}`.reasoning_efforts needs reasoning_control = true"
        )));
    }
    // Mirrors the listing rule: an output limit must fit the context window.
    if let (Some(context), Some(output)) = (declared.context_tokens, declared.max_output_tokens)
        && output > context
    {
        return Err(invalid(&format!(
            "`{alias}`.max_output_tokens must not exceed its context_tokens"
        )));
    }
    Ok(declared)
}

/// Operator spelling only (`off`, not Kanata's wire `none`), sorted.
fn efforts(value: &Value) -> Option<Vec<Effort>> {
    let items = value.as_array().filter(|items| !items.is_empty())?;
    let mut efforts = BTreeSet::new();
    for item in items {
        let text = item.as_str()?;
        let effort = Effort::parse(text).filter(|effort| effort.as_str() == text)?;
        if !efforts.insert(effort) {
            return None;
        }
    }
    Some(efforts.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Result<BTreeMap<String, ModelCapabilities>, String> {
        parse(&BTreeMap::from([(KEY.to_owned(), text.to_owned())]))
            .map_err(|error| error.to_string())
    }

    #[test]
    fn declarations_fill_named_fields_over_the_minimal_profile() {
        let declared = run(r#"{
            "plain/full": {
                "structured_output": true, "function_tools": true,
                "sampling_controls": true, "reasoning_control": true,
                "reasoning_efforts": ["high", "off", "low"],
                "context_tokens": 32768, "max_output_tokens": 4096
            },
            "plain/reasoning": {"output_limit": "max_completion_tokens"},
            "plain/sized": {"output_limit": "max_tokens"},
            "plain/bare": {}
        }"#)
        .unwrap();
        assert_eq!(
            declared["plain/full"],
            ModelCapabilities {
                structured_output: true,
                function_tools: true,
                sampling_controls: true,
                reasoning_control: true,
                reasoning_efforts: Some(vec![Effort::Off, Effort::Low, Effort::High]),
                context_tokens: Some(32_768),
                max_output_tokens: Some(4096),
                ..ModelCapabilities::minimal()
            }
        );
        assert_eq!(
            declared["plain/reasoning"].output_limit,
            Some(OutputLimit::MaxCompletionTokens)
        );
        assert_eq!(
            declared["plain/sized"].output_limit,
            Some(OutputLimit::MaxTokens)
        );
        // Absent: today's rule (`max_tokens` only with sampling controls).
        assert_eq!(declared["plain/full"].output_limit, None);
        assert_eq!(declared["plain/bare"], ModelCapabilities::minimal());
        assert!(parse(&BTreeMap::new()).unwrap().is_empty());
        assert!(run("{}").unwrap().is_empty());
    }

    #[test]
    fn unknown_fields_and_bad_values_refuse_naming_the_alias_and_field() {
        for (bad, error) in [
            (
                r#"["a"]"#,
                "KANADE_MODEL_CAPABILITIES must be a JSON object of model aliases to capability tables",
            ),
            (
                r#"{"": {}}"#,
                "KANADE_MODEL_CAPABILITIES keys must be model aliases (printable ASCII, at most 200 bytes)",
            ),
            (
                r#"{"a b": {}}"#,
                "KANADE_MODEL_CAPABILITIES keys must be model aliases (printable ASCII, at most 200 bytes)",
            ),
            (
                r#"{"m": true}"#,
                "KANADE_MODEL_CAPABILITIES `m` must be a table of capability fields",
            ),
            (
                r#"{"m": {"streaming": true}}"#,
                "KANADE_MODEL_CAPABILITIES `m` has an unknown field `streaming`; declare \
                 structured_output, function_tools, sampling_controls, reasoning_control, \
                 reasoning_efforts, context_tokens, max_output_tokens or output_limit",
            ),
            (
                r#"{"m": {"trust_zone": "local"}}"#,
                "KANADE_MODEL_CAPABILITIES `m` has an unknown field `trust_zone`; declare \
                 structured_output, function_tools, sampling_controls, reasoning_control, \
                 reasoning_efforts, context_tokens, max_output_tokens or output_limit",
            ),
            (
                r#"{"m": {"output_limit": "omitted"}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.output_limit must be \"max_tokens\" or \
                 \"max_completion_tokens\"",
            ),
            (
                r#"{"m": {"output_limit": true}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.output_limit must be \"max_tokens\" or \
                 \"max_completion_tokens\"",
            ),
            (
                r#"{"m": {"function_tools": "yes"}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.function_tools must be true or false",
            ),
            (
                r#"{"m": {"context_tokens": 0}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.context_tokens must be a token count from 1 to 4294967295",
            ),
            (
                r#"{"m": {"context_tokens": 4294967296}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.context_tokens must be a token count from 1 to 4294967295",
            ),
            (
                r#"{"m": {"max_output_tokens": 1048577}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.max_output_tokens must be a token count from 1 to 1048576",
            ),
            (
                r#"{"m": {"context_tokens": 4096, "max_output_tokens": 8192}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.max_output_tokens must not exceed its context_tokens",
            ),
            (
                r#"{"m": {"reasoning_efforts": ["low"]}}"#,
                "KANADE_MODEL_CAPABILITIES `m`.reasoning_efforts needs reasoning_control = true",
            ),
        ] {
            assert_eq!(run(bad).unwrap_err(), error, "{bad}");
        }
        for efforts in [
            r#"[]"#,
            r#"["none"]"#,
            r#"["High"]"#,
            r#"["low","low"]"#,
            r#""low""#,
        ] {
            let bad = format!(
                r#"{{"m": {{"reasoning_control": true, "reasoning_efforts": {efforts}}}}}"#
            );
            assert_eq!(
                run(&bad).unwrap_err(),
                "KANADE_MODEL_CAPABILITIES `m`.reasoning_efforts must be a non-empty list of \
                 distinct levels: off, minimal, low, medium, high, xhigh or max",
                "{efforts}"
            );
        }
    }
}
