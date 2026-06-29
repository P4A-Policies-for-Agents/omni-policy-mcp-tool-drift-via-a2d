// Copyright 2026 Salesforce, Inc. All rights reserved.

//! diff_tool — unchanged / descriptor-drift / unpinned classification.

mod common;

use omni_policy_mcp_tool_drift_via_a2d::spec::{diff_tool, SpecCache, ToolVerdict};

#[test]
fn unchanged_tool_is_unchanged() {
    let spec = SpecCache::from_descriptors("v1", 0, &[common::tool("get_user", "lookup")]);
    let runtime = common::tool("get_user", "lookup");
    assert_eq!(diff_tool(&spec, &runtime), ToolVerdict::Unchanged);
}

#[test]
fn description_change_is_drift() {
    let spec = SpecCache::from_descriptors("v1", 0, &[common::tool("get_user", "safe")]);
    let runtime = common::tool("get_user", "POISONED");
    assert_eq!(diff_tool(&spec, &runtime), ToolVerdict::DescriptorDrift);
}

#[test]
fn new_tool_is_unpinned() {
    let spec = SpecCache::from_descriptors("v1", 0, &[common::tool("get_user", "safe")]);
    let runtime = common::tool("new_tool", "");
    assert_eq!(diff_tool(&spec, &runtime), ToolVerdict::UnpinnedTool);
}

#[test]
fn schema_change_is_drift() {
    let pinned = serde_json::json!({
        "name": "get_user",
        "description": "lookup",
        "inputSchema": {"type": "object", "properties": {"a": {"type": "string"}}},
    });
    let runtime = serde_json::json!({
        "name": "get_user",
        "description": "lookup",
        "inputSchema": {"type": "object", "properties": {"a": {"type": "number"}}},
    });
    let spec = SpecCache::from_descriptors("v1", 0, &[pinned]);
    assert_eq!(diff_tool(&spec, &runtime), ToolVerdict::DescriptorDrift);
}
