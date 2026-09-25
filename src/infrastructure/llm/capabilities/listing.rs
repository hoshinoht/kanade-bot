use std::{collections::BTreeSet, fmt};

use serde_json::{Map, Value};

use super::{AdmissionLimits, Effort, ModelCapabilities, TrustZone};

/// Longest alias kept from a listing; matches the 64 KiB metadata bound.
const MAX_ALIAS_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    /// Parsed `kanata` metadata; `None` when absent or not an object.
    pub capabilities: Option<ModelCapabilities>,
}

impl fmt::Debug for ListedModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListedModel")
            .field("id_bytes", &self.id.len())
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

/// Entries of an OpenAI-style `GET /v1/models` body in listing order, or `None`
/// when the body is not a model list. Unknown fields are ignored; unusable entries
/// are skipped and a repeated id keeps its first entry.
pub fn parse_models_list(body: &[u8]) -> Option<Vec<ListedModel>> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let items = value.get("data")?.as_array()?;
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() || id.len() > MAX_ALIAS_BYTES || !seen.insert(id) {
            continue;
        }
        models.push(ListedModel {
            id: id.to_owned(),
            capabilities: item.get("kanata").and_then(Value::as_object).map(metadata),
        });
    }
    Some(models)
}

fn metadata(meta: &Map<String, Value>) -> ModelCapabilities {
    let flag = |key: &str, default: bool| meta.get(key).and_then(Value::as_bool).unwrap_or(default);
    ModelCapabilities {
        operations: meta
            .get("operations")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|name| name.len() <= MAX_ALIAS_BYTES)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default(),
        structured_output: flag("structured_output", false),
        sampling_controls: flag("sampling_controls", false),
        reasoning_control: flag("reasoning_control", false),
        function_tools: flag("function_tools", true),
        streaming: flag("streaming", false),
        trust_zone: meta
            .get("trust_zone")
            .and_then(Value::as_str)
            .and_then(|zone| match zone {
                "local" => Some(TrustZone::Local),
                "private_network" => Some(TrustZone::PrivateNetwork),
                "external" => Some(TrustZone::External),
                _ => None,
            }),
        reasoning_efforts: meta
            .get("reasoning_efforts")
            .and_then(Value::as_array)
            .map(|items| {
                let mut efforts: Vec<Effort> = items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(Effort::parse)
                    .collect();
                efforts.sort();
                efforts.dedup();
                efforts
            }),
        context_tokens: meta.get("context_tokens").and_then(as_u32),
        admission: meta
            .get("admission")
            .and_then(Value::as_object)
            .and_then(admission),
    }
}

fn as_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|number| u32::try_from(number).ok())
}

/// Needs a usable `max_in_flight`; other fields are optional.
fn admission(meta: &Map<String, Value>) -> Option<AdmissionLimits> {
    Some(AdmissionLimits {
        max_in_flight: meta.get("max_in_flight").and_then(as_u32)?,
        max_queue: meta.get("max_queue").and_then(as_u32),
        queue_ms: meta.get("queue_ms").and_then(Value::as_u64),
        adapter_max_in_flight: meta.get("adapter_max_in_flight").and_then(as_u32),
    })
}
