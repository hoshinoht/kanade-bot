use std::io::{self, Write};

use jsonschema::{Draft, Retrieve, Uri};
use serde::Serialize;
use serde_json::Value;

use super::{ErrorCode, LlmError};

pub(crate) struct ValueBounds {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

pub(crate) fn encoded_size<T: Serialize + ?Sized>(
    value: &T,
    max_bytes: usize,
    code: ErrorCode,
    reason: &str,
) -> Result<usize, LlmError> {
    let mut writer = CappedWriter {
        limit: max_bytes,
        written: 0,
    };
    serde_json::to_writer(&mut writer, value).map_err(|_| LlmError::new(code, reason))?;
    Ok(writer.written)
}

pub(crate) fn preflight_schema(schema: &Value, bounds: &ValueBounds) -> Result<(), LlmError> {
    inspect(schema, bounds, ErrorCode::RequestInvalid, "schema")?;
    reject_nonlocal_references(schema, bounds)
}

pub(crate) fn validate_schema(schema: &Value, bounds: &ValueBounds) -> Result<(), LlmError> {
    preflight_schema(schema, bounds)?;
    compile(schema)?;
    Ok(())
}

pub(crate) fn inspect_json(
    text: &str,
    bounds: &ValueBounds,
    code: ErrorCode,
    reason: &str,
) -> Result<(), LlmError> {
    if text.len() > bounds.max_bytes {
        return Err(LlmError::new(code, reason));
    }
    let value: Value = serde_json::from_str(text).map_err(|_| LlmError::new(code, reason))?;
    inspect(&value, bounds, code, reason)?;
    encoded_size(&value, bounds.max_bytes, code, reason)?;
    Ok(())
}

pub(crate) fn parse_and_validate(
    schema: &Value,
    text: &str,
    value_bounds: &ValueBounds,
    schema_bounds: &ValueBounds,
) -> Result<(), LlmError> {
    if text.len() > value_bounds.max_bytes {
        return Err(LlmError::new(ErrorCode::InvalidOutput, "output-size"));
    }
    let value: Value = serde_json::from_str(text)
        .map_err(|_| LlmError::new(ErrorCode::InvalidOutput, "malformed-json"))?;
    inspect(&value, value_bounds, ErrorCode::InvalidOutput, "output")?;
    encoded_size(
        &value,
        value_bounds.max_bytes,
        ErrorCode::InvalidOutput,
        "output-size",
    )?;
    validate_schema(schema, schema_bounds)?;
    if compile(schema)?.is_valid(&value) {
        Ok(())
    } else {
        Err(LlmError::new(ErrorCode::InvalidOutput, "schema-mismatch"))
    }
}

pub(crate) fn inspect(
    value: &Value,
    bounds: &ValueBounds,
    code: ErrorCode,
    reason: &str,
) -> Result<usize, LlmError> {
    let mut bytes = 0usize;
    let mut nodes = 0usize;
    let mut work = vec![(value, 1usize)];
    while let Some((current, depth)) = work.pop() {
        nodes = nodes
            .checked_add(1)
            .ok_or_else(|| LlmError::new(code, reason))?;
        if nodes > bounds.max_nodes || depth > bounds.max_depth {
            return Err(LlmError::new(code, reason));
        }
        match current {
            Value::Null => add(&mut bytes, 4, bounds, code, reason)?,
            Value::Bool(_) => add(&mut bytes, 5, bounds, code, reason)?,
            Value::Number(number) => {
                add(&mut bytes, number.to_string().len(), bounds, code, reason)?
            }
            Value::String(string) => add(&mut bytes, string.len(), bounds, code, reason)?,
            Value::Array(values) => {
                add(&mut bytes, 2, bounds, code, reason)?;
                let available = bounds
                    .max_nodes
                    .saturating_sub(nodes)
                    .saturating_sub(work.len());
                if values.len() > available {
                    return Err(LlmError::new(code, reason));
                }
                for child in values {
                    work.push((child, depth + 1));
                }
            }
            Value::Object(map) => {
                add(&mut bytes, 2, bounds, code, reason)?;
                let available = bounds
                    .max_nodes
                    .saturating_sub(nodes)
                    .saturating_sub(work.len());
                if map.len() > available {
                    return Err(LlmError::new(code, reason));
                }
                for (key, child) in map {
                    add(&mut bytes, key.len(), bounds, code, reason)?;
                    work.push((child, depth + 1));
                }
            }
        }
    }
    Ok(bytes)
}

fn add(
    total: &mut usize,
    amount: usize,
    bounds: &ValueBounds,
    code: ErrorCode,
    reason: &str,
) -> Result<(), LlmError> {
    *total = total
        .checked_add(amount)
        .ok_or_else(|| LlmError::new(code, reason))?;
    if *total > bounds.max_bytes {
        Err(LlmError::new(code, reason))
    } else {
        Ok(())
    }
}

struct CappedWriter {
    limit: usize,
    written: usize,
}

impl Write for CappedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self
            .written
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("encoded-size"))?;
        if written > self.limit {
            return Err(io::Error::other("encoded-size"));
        }
        self.written = written;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn reject_nonlocal_references(schema: &Value, bounds: &ValueBounds) -> Result<(), LlmError> {
    let mut work = vec![schema];
    let mut seen = 0usize;
    while let Some(value) = work.pop() {
        seen = seen
            .checked_add(1)
            .ok_or_else(|| LlmError::new(ErrorCode::RequestInvalid, "schema"))?;
        if seen > bounds.max_nodes {
            return Err(LlmError::new(ErrorCode::RequestInvalid, "schema"));
        }
        match value {
            Value::Object(map) => {
                let available = bounds
                    .max_nodes
                    .saturating_sub(seen)
                    .saturating_sub(work.len());
                if map.len() > available {
                    return Err(LlmError::new(ErrorCode::RequestInvalid, "schema"));
                }
                for (key, child) in map {
                    if matches!(key.as_str(), "$id" | "$dynamicRef" | "$recursiveRef")
                        || (key == "$ref"
                            && child
                                .as_str()
                                .is_some_and(|reference| !reference.starts_with('#')))
                    {
                        return Err(LlmError::new(ErrorCode::RequestInvalid, "nonlocal-ref"));
                    }
                    work.push(child);
                }
            }
            Value::Array(values) => {
                let available = bounds
                    .max_nodes
                    .saturating_sub(seen)
                    .saturating_sub(work.len());
                if values.len() > available {
                    return Err(LlmError::new(ErrorCode::RequestInvalid, "schema"));
                }
                for child in values {
                    work.push(child);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn compile(schema: &Value) -> Result<jsonschema::Validator, LlmError> {
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .with_retriever(RejectRetriever)
        .build(schema)
        .map_err(|_| LlmError::new(ErrorCode::RequestInvalid, "invalid-schema"))
}
struct RejectRetriever;
impl Retrieve for RejectRetriever {
    fn retrieve(&self, _: &Uri<String>) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(Box::new(io::Error::other("external references disabled")))
    }
}
