// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Decision-source parsing — cache / remote-pdp / hybrid.

mod common;

use omni_policy_mcp_tool_drift_via_a2d::config::DecisionSource;

#[test]
fn cache_parses() {
    assert_eq!(DecisionSource::parse("cache").unwrap(), DecisionSource::Cache);
}

#[test]
fn remote_pdp_parses_both_spellings() {
    assert_eq!(DecisionSource::parse("remote-pdp").unwrap(), DecisionSource::RemotePdp);
    assert_eq!(DecisionSource::parse("remote_pdp").unwrap(), DecisionSource::RemotePdp);
}

#[test]
fn hybrid_parses() {
    assert_eq!(DecisionSource::parse("hybrid").unwrap(), DecisionSource::Hybrid);
}

#[test]
fn unknown_source_is_rejected() {
    assert!(DecisionSource::parse("local-only").is_err());
}
