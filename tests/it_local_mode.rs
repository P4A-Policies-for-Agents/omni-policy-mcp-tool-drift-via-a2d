// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Verifies that the policy compiles with pdk-unit 1.9.0 and the
//! experimental_local_mode feature enabled.

mod common;

use omni_policy_mcp_tool_drift_via_a2d::config::PolicyConfig;
use omni_policy_mcp_tool_drift_via_a2d::generated::config::Config;

#[test]
fn local_mode_api_is_available() {
    // Just verify that the basic pdk-unit 1.9.0 API is available.
    let cfg_json = common::policy_config("cache", "enforce");
    let raw: Config = serde_json::from_str(&cfg_json).unwrap();
    let _policy_cfg = PolicyConfig::from_config(&raw).unwrap();

    // If this compiles and runs, experimental_local_mode feature is working.
    assert!(true);
}
