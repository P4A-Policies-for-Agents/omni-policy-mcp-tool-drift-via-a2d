// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Each policy_config()-style permutation deserializes through the
//! generated config and into PolicyConfig.

mod common;

use omni_policy_mcp_tool_drift_via_a2d::config::{DecisionSource, Mode, PolicyConfig};
use omni_policy_mcp_tool_drift_via_a2d::generated::config::Config;

fn load(source: &str, mode: &str) -> PolicyConfig {
    let raw: Config = serde_json::from_str(&common::policy_config(source, mode)).unwrap();
    PolicyConfig::from_config(&raw).unwrap()
}

#[test]
fn cache_enforce_loads() {
    let c = load("cache", "enforce");
    assert_eq!(c.decision.source, DecisionSource::Cache);
    assert_eq!(c.mode, Mode::Enforce);
}

#[test]
fn remote_pdp_observe_loads() {
    let c = load("remote-pdp", "observe");
    assert_eq!(c.decision.source, DecisionSource::RemotePdp);
    assert_eq!(c.mode, Mode::Observe);
}

#[test]
fn hybrid_warn_loads() {
    let c = load("hybrid", "warn");
    assert_eq!(c.decision.source, DecisionSource::Hybrid);
    assert_eq!(c.mode, Mode::Warn);
    assert!((c.decision.hybrid_sample_rate - 0.1).abs() < f64::EPSILON);
}
