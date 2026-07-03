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
pub mod debounce;
pub mod evidence;
pub mod generated;
pub mod jsonrpc;
pub mod sampler;
pub mod spec;
pub mod sse;

use std::cell::RefCell;
use std::rc::Rc;

use anyhow::anyhow;
use pdk::cache::CacheBuilder;
use pdk::hl::*;
use pdk::logger;

use crate::config::{DecisionSource, Mode, PolicyConfig};
use crate::debounce::{now_epoch_secs, Debouncer};
use crate::evidence::{Decision, DecisionSourceTag, DetectionClass, Event, Severity};
use crate::generated::config::Config;
use crate::spec::{diff_tool, SpecCache, ToolVerdict};

#[derive(Clone)]
struct PolicyState {
    cfg: Rc<PolicyConfig>,
    spec: Rc<RefCell<Option<SpecCache>>>,
    debouncer: Rc<RefCell<Debouncer>>,
}

fn emit_debounced(event: Event<'_>, state: &PolicyState, now_secs: u64) {
    let tool_key = event.tool_name.unwrap_or("<policy>");
    let class_label = event.class.debounce_label();
    if state
        .debouncer
        .borrow_mut()
        .should_emit(tool_key, class_label, now_secs)
    {
        event.emit();
    }
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

const CONTENT_LENGTH_HEADER: &str = "content-length";

async fn response_filter(
    resp_state: ResponseState,
    state: PolicyState,
) {
    let headers_state = resp_state.into_headers_state().await;
    let ct = headers_state
        .handler()
        .headers()
        .into_iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v)
        .unwrap_or_default();

    let is_sse = ct.contains("text/event-stream");
    let is_json = ct.contains("application/json");
    if !is_sse && !is_json {
        return;
    }

    // Strip content-length on the headers handler BEFORE moving to body state.
    headers_state.handler().remove_header(CONTENT_LENGTH_HEADER);

    let body_state = headers_state.into_body_state().await;
    let body = body_state.handler().body().to_vec();

    let rewritten: Option<Vec<u8>> = if is_sse {
        enforce_sse(&body, &state)
    } else {
        enforce_json(&body, &state)
    };

    if let Some(new_body) = rewritten {
        let _ = body_state.handler().set_body(&new_body);
    }
}

/// Parse the SSE body, apply policy to any `tools/list` response, re-emit.
/// Returns `None` when no event was mutated (byte-perfect pass-through).
fn enforce_sse(body: &[u8], state: &PolicyState) -> Option<Vec<u8>> {
    let mut events = sse::parse(body);
    let mut mutated = false;
    for ev in events.iter_mut() {
        let Some(data) = ev.data.as_deref() else {
            continue;
        };
        let Ok(resp) = serde_json::from_str::<jsonrpc::JsonRpcResponse>(data) else {
            continue;
        };
        if let Some(new_resp) = apply_policy(&resp, state) {
            let Ok(new_data) = serde_json::to_string(&new_resp) else {
                continue;
            };
            ev.data = Some(new_data);
            mutated = true;
        }
    }
    if !mutated {
        return None;
    }
    Some(sse::serialize(&events))
}

/// Plain-JSON transport counterpart to `enforce_sse`.
fn enforce_json(body: &[u8], state: &PolicyState) -> Option<Vec<u8>> {
    let resp: jsonrpc::JsonRpcResponse = serde_json::from_slice(body).ok()?;
    let new_resp = apply_policy(&resp, state)?;
    serde_json::to_vec(&new_resp).ok()
}

/// Run the drift-detection logic against a single JSON-RPC response.
///
/// Returns `Some(new_resp)` when Enforce mode stripped at least one tool.
/// Returns `None` when the response is not a successful `tools/list`, the
/// mode is Observe/Warn (evidence still emitted), or every tool passed.
fn apply_policy(
    resp: &jsonrpc::JsonRpcResponse,
    state: &PolicyState,
) -> Option<jsonrpc::JsonRpcResponse> {
    if resp.id.is_none() || matches!(resp.id, Some(serde_json::Value::Null)) {
        return None;
    }
    let tools = jsonrpc::extract_tools_array(resp)?;

    let now_secs = now_epoch_secs();

    let spec_borrow = state.spec.borrow().clone();
    let Some(spec) = spec_borrow else {
        let severity = if state.cfg.fail_open.on_spec_unavailable {
            Severity::Warning
        } else {
            Severity::Critical
        };
        emit_debounced(
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
            },
            state,
            now_secs,
        );
        return None;
    };

    let mut kept: Vec<serde_json::Value> = Vec::with_capacity(tools.len());
    let mut stripped_any = false;
    for tool in tools.iter() {
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
            ToolVerdict::DescriptorDrift => (
                DetectionClass::DescriptorDrift,
                Severity::Critical,
                state.cfg.enforce.exact_match,
            ),
            ToolVerdict::UnpinnedTool => (
                DetectionClass::UnpinnedTool,
                Severity::Warning,
                !state.cfg.enforce.allow_added_tools,
            ),
        };
        emit_debounced(
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
            },
            state,
            now_secs,
        );
        if would_block && matches!(state.cfg.mode, Mode::Enforce) {
            stripped_any = true;
        } else {
            kept.push(tool.clone());
        }
    }

    let runtime_names: std::collections::HashSet<&str> = kept
        .iter()
        .filter_map(|t| t.get("name").and_then(|v| v.as_str()))
        .collect();
    for spec_name in spec.tools.keys() {
        if !runtime_names.contains(spec_name.as_str()) && state.cfg.enforce.allow_removed_tools {
            emit_debounced(
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
                },
                state,
                now_secs,
            );
        }
    }

    if matches!(state.cfg.mode, Mode::Enforce) && stripped_any {
        Some(rewrite_tools_list(resp, kept))
    } else {
        None
    }
}

fn rewrite_tools_list(
    resp: &jsonrpc::JsonRpcResponse,
    kept: Vec<serde_json::Value>,
) -> jsonrpc::JsonRpcResponse {
    let mut new_resp = resp.clone();
    let mut result = new_resp.result.unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = result.as_object_mut() {
        map.insert("tools".into(), serde_json::Value::Array(kept));
    }
    new_resp.result = Some(result);
    new_resp
}

#[entrypoint]
pub async fn configure(
    launcher: Launcher,
    Configuration(bytes): Configuration,
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
        debouncer: Rc::new(RefCell::new(Debouncer::default())),
    };

    let filter = on_response(move |resp: ResponseState| {
        let s = state.clone();
        async move { response_filter(resp, s).await }
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
