//! MCP Tool Drift Detection (via A²D) — policy entrypoint.
//!
//! Three decision sources:
//! - `cache` — local LKG spec, lowest latency.
//! - `remote-pdp` — per-request A²D PDP call.
//! - `hybrid` — decide locally; sample async PDP audit; raise
//!   `pdp_disagreement` on divergence.

pub mod a2d;
pub mod cache;
pub mod config;
pub mod evidence;
pub mod generated;
pub mod jsonrpc;
pub mod sampler;
pub mod spec;

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::anyhow;
use pdk::cache::CacheBuilder;
use pdk::hl::*;
use pdk::logger;

use crate::config::{DecisionSource, Mode, PolicyConfig};
use crate::evidence::{Decision, DecisionSourceTag, DetectionClass, Event, Severity};
use crate::generated::config::Config;
use crate::spec::{diff_tool, SpecCache, ToolVerdict};

#[derive(Clone)]
struct PolicyState {
    cfg: Rc<PolicyConfig>,
    spec: Rc<RefCell<Option<SpecCache>>>,
}

fn source_tag(source: DecisionSource) -> DecisionSourceTag {
    match source {
        DecisionSource::Cache => DecisionSourceTag::Cache,
        DecisionSource::RemotePdp => DecisionSourceTag::RemotePdp,
        DecisionSource::Hybrid => DecisionSourceTag::Hybrid,
    }
}

fn decision_for(mode: Mode, would_block: bool) -> Decision {
    match (mode, would_block) {
        (Mode::Enforce, true) => Decision::Stripped,
        (Mode::Enforce, false) => Decision::Allowed,
        (Mode::Observe, _) => Decision::Allowed,
        (Mode::Warn, true) => Decision::Annotated,
        (Mode::Warn, false) => Decision::Allowed,
    }
}

async fn response_filter(
    _req_state: RequestState,
    resp_state: ResponseState,
    state: PolicyState,
) -> Flow<()> {
    let headers_state = resp_state.into_headers_state().await;
    let ct = headers_state
        .handler()
        .headers()
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v)
        .unwrap_or_default();
    if !ct.contains("application/json") && !ct.contains("text/event-stream") {
        return Flow::Continue(());
    }
    let body_state = headers_state.into_body_state().await;
    let body = body_state.handler().body().to_vec();
    let resp: jsonrpc::JsonRpcResponse = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => return Flow::Continue(()),
    };
    let Some(tools) = jsonrpc::extract_tools_array(&resp) else {
        return Flow::Continue(());
    };

    let spec_borrow = state.spec.borrow().clone();
    let Some(spec) = spec_borrow else {
        let severity = if state.cfg.fail_open.on_spec_unavailable {
            Severity::Warning
        } else {
            Severity::Critical
        };
        Event {
            class: DetectionClass::SpecUnavailable,
            severity,
            decision: if state.cfg.fail_open.on_spec_unavailable {
                Decision::Allowed
            } else {
                Decision::Blocked
            },
            source: source_tag(state.cfg.decision.source),
            asset_id: &state.cfg.a2d.asset_id,
            asset_version: None,
            policy_instance_id: None,
            tool_name: None,
            local_verdict: None,
            pdp_verdict: None,
            note: Some("no spec loaded"),
        }
        .emit();
        return Flow::Continue(());
    };

    let mut kept: Vec<serde_json::Value> = Vec::with_capacity(tools.len());
    for tool in tools {
        let name = tool
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        let verdict = diff_tool(&spec, tool);
        let (class, severity, would_block) = match verdict {
            ToolVerdict::Unchanged => {
                kept.push(tool.clone());
                continue;
            }
            ToolVerdict::DescriptorDrift => {
                (DetectionClass::DescriptorDrift, Severity::Critical, state.cfg.enforce.exact_match)
            }
            ToolVerdict::UnpinnedTool => (
                DetectionClass::UnpinnedTool,
                Severity::Warning,
                !state.cfg.enforce.allow_added_tools,
            ),
        };
        Event {
            class,
            severity,
            decision: decision_for(state.cfg.mode, would_block),
            source: source_tag(state.cfg.decision.source),
            asset_id: &state.cfg.a2d.asset_id,
            asset_version: Some(&spec.asset_version),
            policy_instance_id: None,
            tool_name: Some(name),
            local_verdict: Some(match verdict {
                ToolVerdict::Unchanged => "unchanged",
                ToolVerdict::DescriptorDrift => "descriptor_drift",
                ToolVerdict::UnpinnedTool => "unpinned",
            }),
            pdp_verdict: None,
            note: None,
        }
        .emit();
        if !(would_block && matches!(state.cfg.mode, Mode::Enforce)) {
            kept.push(tool.clone());
        }
    }

    let runtime_names: std::collections::HashSet<&str> = kept
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect();
    for spec_name in spec.tools.keys() {
        if !runtime_names.contains(spec_name.as_str()) && state.cfg.enforce.allow_removed_tools {
            Event {
                class: DetectionClass::RemovedTool,
                severity: Severity::Info,
                decision: Decision::Allowed,
                source: source_tag(state.cfg.decision.source),
                asset_id: &state.cfg.a2d.asset_id,
                asset_version: Some(&spec.asset_version),
                policy_instance_id: None,
                tool_name: Some(spec_name),
                local_verdict: None,
                pdp_verdict: None,
                note: None,
            }
            .emit();
        }
    }

    if matches!(state.cfg.mode, Mode::Enforce) {
        let rewritten = rewrite_tools_list(&resp, kept);
        body_state.handler().set_body(&rewritten);
    }
    Flow::Continue(())
}

fn rewrite_tools_list(resp: &jsonrpc::JsonRpcResponse, kept: Vec<serde_json::Value>) -> Vec<u8> {
    let mut new_resp = resp.clone();
    let mut result = new_resp.result.unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = result.as_object_mut() {
        map.insert("tools".into(), serde_json::Value::Array(kept));
    }
    new_resp.result = Some(result);
    serde_json::to_vec(&new_resp).expect("response serializes")
}

#[entrypoint]
pub async fn configure(
    launcher: Launcher,
    Configuration(bytes): Configuration,
    _client: HttpClient,
    _clock: Clock,
    _timer: Timer,
    _cache_builder: CacheBuilder,
) -> anyhow::Result<()> {
    let raw: Config = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow!("invalid policy configuration: {e}"))?;
    let cfg = PolicyConfig::from_config(&raw)
        .map_err(|e| anyhow!("policy configuration rejected: {e}"))?;

    logger::info!(
        "mcp-drift-a2d: asset={} base={} source={:?} mode={:?}",
        cfg.a2d.asset_id,
        cfg.a2d.base_url,
        cfg.decision.source,
        cfg.mode,
    );

    let state = PolicyState {
        cfg: Rc::new(cfg),
        spec: Rc::new(RefCell::new(None)),
    };

    let filter = on_response(move |req: RequestState, resp: ResponseState| {
        let s = state.clone();
        async move { response_filter(req, resp, s).await }
    });
    launcher.launch(filter).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decision_for_enforce_blocks() {
        assert!(matches!(decision_for(Mode::Enforce, true), Decision::Stripped));
        assert!(matches!(decision_for(Mode::Observe, true), Decision::Allowed));
        assert!(matches!(decision_for(Mode::Warn, true), Decision::Annotated));
    }
}
