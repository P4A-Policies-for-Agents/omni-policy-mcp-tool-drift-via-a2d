// Copyright 2026 Salesforce, Inc. All rights reserved.

//! A²D endpoint construction — spec / validate / evidence.

mod common;

use omni_policy_mcp_tool_drift_via_a2d::a2d::A2dRef;

fn make_ref(base: &str) -> A2dRef {
    A2dRef {
        base_url: base.into(),
        asset_id: "abc".into(),
        api_key: "key".into(),
        path_prefix: String::new(),
    }
}

#[test]
fn all_three_urls_share_asset_root() {
    let r = make_ref("https://a2d-ai.com");
    assert_eq!(r.spec_url(), "https://a2d-ai.com/api/platform/abc/mcp/spec");
    assert_eq!(r.validate_url(), "https://a2d-ai.com/api/platform/abc/mcp/validate");
    assert_eq!(r.evidence_url(), "https://a2d-ai.com/api/platform/abc/mcp/evidence");
}

#[test]
fn trailing_slash_is_handled() {
    let r = make_ref("https://a2d-ai.com/");
    assert_eq!(r.spec_url(), "https://a2d-ai.com/api/platform/abc/mcp/spec");
}

#[test]
fn loopback_prefix_is_prepended() {
    let mut r = make_ref("http://127.0.0.1:8081");
    r.path_prefix = "/a2d-pin".into();
    assert_eq!(
        r.spec_url(),
        "http://127.0.0.1:8081/a2d-pin/api/platform/abc/mcp/spec"
    );
}
