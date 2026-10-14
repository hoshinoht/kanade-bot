//! `KANADE_MODEL_GROUPS`: backend capacity groups as a JSON list of
//! `{name, permits, aliases}` (`kanade.toml` `[[models.groups]]` arrives in
//! this form). Unset keeps the single `gateway` group of `KANADE_MODEL_PERMITS`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::{Error, non_empty};
use crate::infrastructure::llm::setup::CapacityGroup;

pub(super) const GROUPS: &str = "KANADE_MODEL_GROUPS";
const MAX_PERMITS: u32 = 16;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    name: String,
    permits: u32,
    aliases: Vec<String>,
}

fn invalid(rule: &str) -> Error {
    Error::Configuration(format!("{GROUPS} {rule}"))
}

pub(super) fn parse(values: &BTreeMap<String, String>) -> Result<Vec<CapacityGroup>, Error> {
    let Some(text) = non_empty(values, GROUPS) else {
        return Ok(Vec::new());
    };
    let groups: Vec<Group> = serde_json::from_str(text)
        .map_err(|_| invalid("must be a JSON list of {name, permits, aliases}"))?;
    if !groups.is_empty() && non_empty(values, "KANADE_MODEL_PERMITS").is_some() {
        return Err(Error::Configuration(format!(
            "KANADE_MODEL_PERMITS and {GROUPS} are exclusive; set permits per group"
        )));
    }
    let mut names = BTreeSet::new();
    let mut owned = BTreeSet::new();
    groups
        .into_iter()
        .map(|group| {
            let name_ok = !group.name.is_empty()
                && group.name.len() <= 64
                && group
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
            if !name_ok || !names.insert(group.name.clone()) {
                return Err(invalid(
                    "names must be unique, at most 64 letters, digits, `-`, `_` or `.`",
                ));
            }
            if !(1..=MAX_PERMITS).contains(&group.permits) {
                return Err(invalid(&format!(
                    "permits must be between 1 and {MAX_PERMITS}"
                )));
            }
            let aliases_ok = !group.aliases.is_empty()
                && group.aliases.iter().all(|alias| {
                    !alias.is_empty()
                        && alias.len() <= 200
                        && alias.bytes().all(|byte| byte.is_ascii_graphic())
                        && owned.insert(alias.clone())
                });
            if !aliases_ok {
                return Err(invalid(
                    "aliases must be model aliases, at least one per group, each in one group",
                ));
            }
            Ok(CapacityGroup {
                name: group.name,
                permits: group.permits,
                aliases: group.aliases,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str, permits: Option<&str>) -> Result<Vec<CapacityGroup>, String> {
        let mut values = BTreeMap::from([(GROUPS.to_owned(), text.to_owned())]);
        if let Some(permits) = permits {
            values.insert("KANADE_MODEL_PERMITS".into(), permits.into());
        }
        parse(&values).map_err(|error| error.to_string())
    }

    #[test]
    fn groups_parse_and_refuse_without_echoing_values() {
        let groups = run(
            r#"[{"name":"local","permits":3,"aliases":["a","b"]},{"name":"cloud","permits":1,"aliases":["c"]}]"#,
            None,
        )
        .unwrap();
        assert_eq!(groups[0].name, "local");
        assert_eq!(groups[0].aliases, ["a", "b"]);
        assert_eq!(groups[1].permits, 1);
        assert_eq!(run("[]", Some("4")).unwrap(), []);
        for (bad, error) in [
            (
                r#"{"name":"x"}"#,
                "KANADE_MODEL_GROUPS must be a JSON list of {name, permits, aliases}",
            ),
            (
                r#"[{"name":"x","permits":1,"aliases":["a"],"burst":2}]"#,
                "KANADE_MODEL_GROUPS must be a JSON list of {name, permits, aliases}",
            ),
            (
                r#"[{"name":"x y","permits":1,"aliases":["a"]}]"#,
                "KANADE_MODEL_GROUPS names must be unique, at most 64 letters, digits, `-`, `_` or `.`",
            ),
            (
                r#"[{"name":"x","permits":1,"aliases":["a"]},{"name":"x","permits":1,"aliases":["b"]}]"#,
                "KANADE_MODEL_GROUPS names must be unique, at most 64 letters, digits, `-`, `_` or `.`",
            ),
            (
                r#"[{"name":"x","permits":17,"aliases":["a"]}]"#,
                "KANADE_MODEL_GROUPS permits must be between 1 and 16",
            ),
            (
                r#"[{"name":"x","permits":1,"aliases":[]}]"#,
                "KANADE_MODEL_GROUPS aliases must be model aliases, at least one per group, each in one group",
            ),
            (
                r#"[{"name":"x","permits":1,"aliases":["a"]},{"name":"y","permits":1,"aliases":["a"]}]"#,
                "KANADE_MODEL_GROUPS aliases must be model aliases, at least one per group, each in one group",
            ),
        ] {
            assert_eq!(run(bad, None).unwrap_err(), error, "{bad}");
        }
        assert_eq!(
            run(r#"[{"name":"x","permits":1,"aliases":["a"]}]"#, Some("2")).unwrap_err(),
            "KANADE_MODEL_PERMITS and KANADE_MODEL_GROUPS are exclusive; set permits per group"
        );
    }
}
