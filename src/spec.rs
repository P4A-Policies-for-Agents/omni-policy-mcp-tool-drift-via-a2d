//! Canonical-spec cache. The "spec" is the approved set of MCP tool
//! descriptors from A²D. Each descriptor is hashed in canonical form
//! so a runtime descriptor can be compared by hash for O(1) drift
//! detection.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn canonical_hash(tool: &serde_json::Value) -> String {
    let mut canon = serde_json::Map::new();
    for key in ["name", "description", "inputSchema", "outputSchema", "annotations"] {
        if let Some(v) = tool.get(key) {
            canon.insert(key.to_string(), canonical_field(key, v));
        }
    }
    let bytes = serde_json::to_vec(&serde_json::Value::Object(canon))
        .expect("canonical map serializes");
    let mut h = Sha256::new();
    h.update(&bytes);
    hex_encode(&h.finalize())
}

fn canonicalize(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let sorted: BTreeMap<&String, &serde_json::Value> = m.iter().collect();
            let mut out = serde_json::Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k.clone(), canonicalize(v));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(canonicalize).collect())
        }
        other => other.clone(),
    }
}

/// JSON-Schema keys that carry no behavioral contract. Different schema
/// serializers add or drop these freely (e.g. an MCP server that
/// regenerates strict draft-07 schemas at runtime vs. the minimal copy
/// stored at authoring time), so they are removed before hashing to stop
/// cosmetic serialization differences from reading as tool drift.
const SCHEMA_META_KEYS: &[&str] = &["$schema", "$id", "$comment"];

/// Normalize a JSON Schema value for hashing:
/// - drop metadata-only keys (`$schema`, `$id`, `$comment`),
/// - fold the strict-default `additionalProperties: false` to "absent"
///   so a serializer that emits it hashes the same as one that omits it.
///
/// Only provably cosmetic differences are folded. A real contract change
/// — `type`/`properties`/`required`/`enum`/nested `description`, or
/// loosening `additionalProperties` to `true`/a subschema — still changes
/// the hash and is reported as drift.
fn normalize_schema(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if SCHEMA_META_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if k == "additionalProperties" && val.as_bool() == Some(false) {
                    continue;
                }
                out.insert(k.clone(), normalize_schema(val));
            }
            serde_json::Value::Object(out)
        }
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(normalize_schema).collect())
        }
        other => other.clone(),
    }
}

/// Canonicalize one top-level descriptor field. Schema-bearing fields are
/// first run through `normalize_schema` so semantically-equal schemas
/// hash identically regardless of serializer decoration.
fn canonical_field(key: &str, v: &serde_json::Value) -> serde_json::Value {
    match key {
        "inputSchema" | "outputSchema" => canonicalize(&normalize_schema(v)),
        _ => canonicalize(v),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecTool {
    pub name: String,
    pub hash: String,
}

impl SpecTool {
    pub fn from_descriptor(d: &serde_json::Value) -> Option<Self> {
        let name = d.get("name")?.as_str()?.to_string();
        Some(Self { name, hash: canonical_hash(d) })
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpecCache {
    pub asset_version: String,
    pub fetched_at_epoch_secs: u64,
    pub tools: BTreeMap<String, SpecTool>,
}

impl SpecCache {
    pub fn from_descriptors(
        asset_version: &str,
        now_secs: u64,
        descs: &[serde_json::Value],
    ) -> Self {
        let mut tools = BTreeMap::new();
        for d in descs {
            if let Some(t) = SpecTool::from_descriptor(d) {
                tools.insert(t.name.clone(), t);
            }
        }
        Self {
            asset_version: asset_version.to_string(),
            fetched_at_epoch_secs: now_secs,
            tools,
        }
    }
}

/// Diff a runtime tool descriptor against the spec; returns the
/// verdict for that tool only. `None` means "no spec entry by that
/// name."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolVerdict {
    Unchanged,
    DescriptorDrift,
    UnpinnedTool,
}

pub fn diff_tool(spec: &SpecCache, runtime: &serde_json::Value) -> ToolVerdict {
    let Some(name) = runtime.get("name").and_then(|v| v.as_str()) else {
        return ToolVerdict::UnpinnedTool;
    };
    match spec.tools.get(name) {
        None => ToolVerdict::UnpinnedTool,
        Some(spec_tool) if canonical_hash(runtime) == spec_tool.hash => ToolVerdict::Unchanged,
        Some(_) => ToolVerdict::DescriptorDrift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a runtime serializer that adds `$schema` +
    /// `additionalProperties: false` must NOT read as drift against a spec
    /// stored without those decorations.
    #[test]
    fn schema_serializer_decorations_do_not_drift() {
        let pinned = serde_json::json!({
            "name": "t", "description": "x",
            "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}
        });
        let runtime = serde_json::json!({
            "name": "t", "description": "x",
            "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"],
                "additionalProperties": false, "$schema": "http://json-schema.org/draft-07/schema#"}
        });
        assert_eq!(canonical_hash(&pinned), canonical_hash(&runtime));
        let spec = SpecCache::from_descriptors("1", 0, &[pinned]);
        assert_eq!(diff_tool(&spec, &runtime), ToolVerdict::Unchanged);
    }

    fn tool(name: &str, desc: &str) -> serde_json::Value {
        serde_json::json!({"name": name, "description": desc, "inputSchema": {"type": "object"}})
    }

    #[test]
    fn unchanged_tool_reports_unchanged() {
        let spec = SpecCache::from_descriptors("1", 0, &[tool("get_user", "lookup")]);
        assert_eq!(diff_tool(&spec, &tool("get_user", "lookup")), ToolVerdict::Unchanged);
    }

    #[test]
    fn description_drift_detected() {
        let spec = SpecCache::from_descriptors("1", 0, &[tool("get_user", "lookup")]);
        assert_eq!(
            diff_tool(&spec, &tool("get_user", "POISONED")),
            ToolVerdict::DescriptorDrift
        );
    }

    #[test]
    fn missing_from_spec_is_unpinned() {
        let spec = SpecCache::from_descriptors("1", 0, &[tool("get_user", "lookup")]);
        assert_eq!(diff_tool(&spec, &tool("new_tool", "x")), ToolVerdict::UnpinnedTool);
    }
}
