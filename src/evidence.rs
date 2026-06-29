//! Structured evidence events.

use pdk::logger;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectionClass {
    DescriptorDrift,
    UnpinnedTool,
    RemovedTool,
    SpecUnavailable,
    SpecStale,
    PdpUnavailable,
    PdpDisagreement,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allowed,
    Blocked,
    Stripped,
    Annotated,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionSourceTag {
    Cache,
    RemotePdp,
    Hybrid,
}

#[derive(Debug, Clone, Serialize)]
pub struct Event<'a> {
    pub class: DetectionClass,
    pub severity: Severity,
    pub decision: Decision,
    pub source: DecisionSourceTag,
    pub asset_id: &'a str,
    pub asset_version: Option<&'a str>,
    pub policy_instance_id: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub local_verdict: Option<&'a str>,
    pub pdp_verdict: Option<&'a str>,
    pub note: Option<&'a str>,
}

impl<'a> Event<'a> {
    pub fn emit(&self) {
        let json = serde_json::to_string(self).unwrap_or_else(|_| "{}".into());
        match self.severity {
            Severity::Critical => logger::error!("mcp-drift-a2d-evt {}", json),
            Severity::Warning => logger::warn!("mcp-drift-a2d-evt {}", json),
            Severity::Info => logger::info!("mcp-drift-a2d-evt {}", json),
        }
    }
}
