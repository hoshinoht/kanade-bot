//! `kanade.toml` (path in `KANADE_CONFIG`): non-secret settings flattened onto
//! the `KANADE_*` names the typed configs already parse. A non-empty
//! environment variable overrides its key. Unknown keys, wrong types and
//! secret-looking keys stop startup naming the key, never the value.

mod keys;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use toml::{Table, Value};

use self::keys::Kind;
use super::{Error, non_empty};

const CONFIG: &str = "KANADE_CONFIG";
const MAX_BYTES: u64 = 1 << 20;

/// The merged mapping, plus which variables came from the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub values: BTreeMap<String, String>,
    /// Env name → TOML key, only where the file's value is the one in use.
    origins: BTreeMap<String, &'static str>,
}

impl Resolved {
    /// Points a configuration error about a file-supplied variable at its key.
    pub fn annotate(&self, error: Error) -> Error {
        let Error::Configuration(message) = error else {
            return error;
        };
        let first = message.split([' ', ':']).next().unwrap_or_default();
        match self.origins.get(first) {
            Some(key) => Error::Configuration(format!("{message} (kanade.toml `{key}`)")),
            None => Error::Configuration(message),
        }
    }
}

/// Reads `KANADE_CONFIG` when set; otherwise the environment alone.
pub fn resolve(environment: BTreeMap<String, String>) -> Result<Resolved, Error> {
    let Some(path) = non_empty(&environment, CONFIG) else {
        return Ok(Resolved {
            values: environment,
            origins: BTreeMap::new(),
        });
    };
    let unreadable = || Error::Configuration(format!("{CONFIG} could not be read"));
    let file = std::fs::File::open(path).map_err(|_| unreadable())?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(file, MAX_BYTES + 1), &mut text)
        .map_err(|_| unreadable())?;
    if text.len() as u64 > MAX_BYTES {
        return Err(Error::Configuration(format!(
            "{CONFIG} is larger than 1 MiB"
        )));
    }
    merge(&text, environment)
}

fn merge(text: &str, environment: BTreeMap<String, String>) -> Result<Resolved, Error> {
    let table: Table = toml::from_str(text).map_err(|error| {
        // The parser's own message may quote the offending text.
        let line = error.span().map(|span| {
            let before = &text.as_bytes()[..span.start.min(text.len())];
            before.iter().filter(|byte| **byte == b'\n').count() + 1
        });
        Error::Configuration(match line {
            Some(line) => format!("kanade.toml is not valid TOML (line {line})"),
            None => "kanade.toml is not valid TOML".into(),
        })
    })?;
    let mut file = BTreeMap::new();
    flatten(&table, "", &mut file)?;
    let mut resolved = Resolved {
        values: environment,
        origins: BTreeMap::new(),
    };
    for (path, (env, value)) in file {
        if non_empty(&resolved.values, env).is_none() {
            resolved.values.insert(env.to_owned(), value);
            resolved.origins.insert(env.to_owned(), path);
        }
    }
    Ok(resolved)
}

fn flatten(
    table: &Table,
    prefix: &str,
    out: &mut BTreeMap<&'static str, (&'static str, String)>,
) -> Result<(), Error> {
    for (name, value) in table {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };
        if keys::is_retired(&path) {
            return Err(Error::Configuration(format!(
                "kanade.toml key `{path}` is retired; remove it"
            )));
        }
        if keys::looks_secret(name) {
            return Err(secret(&path));
        }
        if let Some((key, env, kind)) = keys::lookup(&path) {
            out.insert(key, (env, convert(value, kind, &path)?));
        } else if keys::is_section(&path) {
            let Value::Table(inner) = value else {
                return Err(wrong(&path, "a table"));
            };
            flatten(inner, &path, out)?;
        } else {
            return Err(Error::Configuration(format!(
                "kanade.toml has an unknown key `{path}`"
            )));
        }
    }
    Ok(())
}

fn secret(path: &str) -> Error {
    Error::Configuration(format!(
        "kanade.toml `{path}` looks like a secret; put it in a file and name that file with a `*_file` key"
    ))
}

fn wrong(path: &str, expected: &str) -> Error {
    Error::Configuration(format!("kanade.toml `{path}` must be {expected}"))
}

fn convert(value: &Value, kind: Kind, path: &str) -> Result<String, Error> {
    let list = |expected: &str, item: &dyn Fn(&Value) -> Option<String>| {
        let Value::Array(items) = value else {
            return Err(wrong(path, expected));
        };
        items
            .iter()
            .map(|value| item(value).ok_or_else(|| wrong(path, expected)))
            .collect::<Result<Vec<_>, _>>()
            .map(|items| items.join(","))
    };
    match kind {
        Kind::Text => value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| wrong(path, "a string")),
        Kind::Int => integer(value).ok_or_else(|| wrong(path, "a non-negative integer")),
        Kind::Flag => value
            .as_bool()
            .map(|flag| if flag { "1" } else { "0" }.to_owned())
            .ok_or_else(|| wrong(path, "true or false")),
        Kind::Id => id(value).ok_or_else(|| wrong(path, "a snowflake string or integer")),
        Kind::Ids => list("a list of snowflake strings or integers", &id),
        Kind::Texts => list("a list of strings without commas", &|value| {
            value
                .as_str()
                .filter(|text| !text.contains(','))
                .map(str::to_owned)
        }),
        Kind::Ints => list("a list of non-negative integers", &integer),
        Kind::Groups => groups(value, path),
        Kind::Context => {
            serde_json::to_string(value).map_err(|_| wrong(path, "a context settings table"))
        }
        Kind::RunLengths => {
            serde_json::to_string(value).map_err(|_| wrong(path, "a run lengths settings table"))
        }
    }
}

fn integer(value: &Value) -> Option<String> {
    value
        .as_integer()
        .filter(|number| *number >= 0)
        .map(|number| number.to_string())
}

fn id(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Integer(number) if *number > 0 => Some(number.to_string()),
        _ => None,
    }
}

/// `[[models.groups]]` → `KANADE_MODEL_GROUPS` JSON; values are checked there.
fn groups(value: &Value, path: &str) -> Result<String, Error> {
    let Value::Array(items) = value else {
        return Err(wrong(path, "an array of tables"));
    };
    let mut groups = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let at = format!("{path}[{index}]");
        let Value::Table(table) = item else {
            return Err(wrong(&at, "a table"));
        };
        let mut group = serde_json::Map::new();
        for (name, value) in table {
            let key = format!("{at}.{name}");
            let json = match name.as_str() {
                "name" => value.as_str().map(serde_json::Value::from),
                "permits" => value
                    .as_integer()
                    .filter(|number| *number >= 0)
                    .map(serde_json::Value::from),
                "aliases" => value.as_array().and_then(|aliases| {
                    aliases
                        .iter()
                        .map(|alias| alias.as_str().map(serde_json::Value::from))
                        .collect::<Option<Vec<_>>>()
                        .map(serde_json::Value::from)
                }),
                _ if keys::looks_secret(name) => return Err(secret(&key)),
                _ => {
                    return Err(Error::Configuration(format!(
                        "kanade.toml has an unknown key `{key}`"
                    )));
                }
            };
            let expected = match name.as_str() {
                "name" => "a string",
                "permits" => "a non-negative integer",
                _ => "a list of strings",
            };
            group.insert(name.clone(), json.ok_or_else(|| wrong(&key, expected))?);
        }
        for required in ["name", "permits", "aliases"] {
            if !group.contains_key(required) {
                return Err(Error::Configuration(format!(
                    "kanade.toml `{at}` needs `{required}`"
                )));
            }
        }
        groups.push(serde_json::Value::Object(group));
    }
    Ok(serde_json::Value::Array(groups).to_string())
}
