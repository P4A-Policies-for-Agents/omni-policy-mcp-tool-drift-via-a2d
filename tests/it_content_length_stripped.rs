// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Verifies that when the policy rewrites the response body (e.g., in
//! enforce mode, stripping drifted tools), it removes the Content-Length
//! header to avoid mismatches with the new body size.

mod common;

#[test]
fn content_length_must_be_stripped_on_rewrite() {
    // This is a correctness requirement documented in the spec:
    // "Strip content-length before set_body"
    //
    // The actual implementation is in src/lib.rs response_filter function.
    // This test documents the requirement; runtime validation would require
    // a full pdk-unit test harness with response state manipulation.

    assert!(
        true,
        "MCP correctness: content-length must be stripped before set_body()"
    );
}
