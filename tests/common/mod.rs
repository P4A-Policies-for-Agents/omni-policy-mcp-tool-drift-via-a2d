// Copyright 2026 Salesforce, Inc. All rights reserved.

//! Helpers shared across integration tests for MCP Tool Drift
//! Detection (via A²D).

#![allow(dead_code)]

use pdk_unit::{Backend, UnitHttpMessage, UnitHttpRequest, UnitHttpResponse};

pub struct RouterBackend {
    inner: Box<dyn Fn(UnitHttpRequest) -> UnitHttpResponse>,
}

impl RouterBackend {
    pub fn new<F: Fn(UnitHttpRequest) -> UnitHttpResponse + 'static>(f: F) -> Self {
        Self { inner: Box::new(f) }
    }
}

impl Backend for RouterBackend {
    fn call(&self, req: UnitHttpRequest) -> UnitHttpResponse {
        (self.inner)(req)
    }
}

pub fn json(status: u32, body: &str) -> UnitHttpResponse {
    UnitHttpResponse::new(status)
        .with_header("content-type", "application/json")
        .with_body(body.as_bytes().to_vec())
}

pub fn tool(name: &str, description: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": {"type": "object"},
    })
}

pub fn tools_list_body(id: u64, tools: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"tools": tools},
    })
    .to_string()
}

pub fn policy_config(source: &str, mode: &str) -> String {
    serde_json::json!({
        "a2d": {
            "baseUrl": "https://a2d-ai.com",
            "assetId": "demo-mcp-asset",
            "apiKeySecretRef": "a2d-api-key",
            "refreshIntervalSec": 300,
            "pdpTimeoutMs": 250,
        },
        "decision": {
            "source": source,
            "hybridSampleRate": 0.1,
        },
        "enforce": {
            "exactMatch": true,
            "allowAddedTools": false,
            "allowRemovedTools": true,
        },
        "evidence": {"reportToA2d": false, "logLocally": true},
        "mode": mode,
        "failOpen": {"onSpecUnavailable": true, "onPdpUnavailable": true},
    })
    .to_string()
}
