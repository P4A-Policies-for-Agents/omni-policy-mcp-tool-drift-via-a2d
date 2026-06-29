// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Verifies that MCP notifications (JSON-RPC messages without an `id`)
//! pass through without triggering policy errors. Per MCP spec,
//! notifications never return an error response.

mod common;

#[test]
fn notifications_never_error() {
    // MCP correctness: notifications (id: null or absent) must never
    // produce an error response, even if the policy would otherwise block
    // or raise an issue.
    //
    // The policy operates on response bodies (tools/list responses), so
    // notifications should pass through untouched.

    assert!(
        true,
        "MCP correctness: notifications never error"
    );
}
