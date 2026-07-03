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
use std::time::Duration;

use anyhow::anyhow;
use pdk::cache::CacheBuilder;
use pdk::hl::*;
use pdk::logger;

use crate::a2d::{A2dClient, A2dRef as A2dTarget};
use crate::config::{DecisionSource, Mode, PolicyConfig};
use crate::debounce::{now_epoch_secs, Debouncer};
use crate::evidence::{Decision, DecisionSourceTag, DetectionClass, Event, Severity};
use crate::generated::config::Config;
use crate::sampler::sample_should_fire;
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

/// Discover the upstream cluster Envoy selected for the current request
/// from the stream properties. This is the cluster the API instance
/// itself proxies to (e.g. `api-instance-20999091.<env>.svc`), which is
/// configured to reach the A²D upstream with the correct `Host`. It is
/// generally populated once routing has picked an upstream — reliably in
/// the response phase, and often in the request phase too.
fn cluster_from_props(props: &StreamProperties) -> Option<String> {
    for path in [&["cluster_name"][..], &["xds", "cluster_name"][..]] {
        if let Some(bytes) = props.read_property(path) {
            if let Ok(s) = std::str::from_utf8(&bytes) {
                let s = s.trim();
                if !s.is_empty() {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

/// Resolve the `Service` used for outbound A²D calls.
///
/// A policy-registered `format: service` upstream is dispatched to a
/// synthetic Envoy cluster whose egress `:authority` is mangled by the
/// gateway's host-rewrite, so Vercel returns `DEPLOYMENT_NOT_FOUND`.
/// Instead we reuse the request's *own* upstream cluster (discovered via
/// the `cluster_name` stream property, or the `x-envoy-decorator-operation`
/// response header as a fallback), which the gateway already routes to
/// A²D with the correct `Host`. We keep the same `baseUrl` authority so
/// the wire `:authority` is correct. If the cluster can't be discovered,
/// `allow_config_fallback` permits the legacy `format: service` path as a
/// last resort.
fn resolve_outbound_service(
    state: &PolicyState,
    props: &StreamProperties,
    decorator: Option<&str>,
    allow_config_fallback: bool,
) -> Option<Service> {
    // Loopback mode: when a pin-path prefix is configured, dispatch to the
    // configured `baseUrl` (the gateway's own internal listener) verbatim
    // and skip upstream-cluster discovery. The prefixed path re-enters
    // through a plain passthrough route whose `auto_host_rewrite` restores
    // the correct A²D `Host`, sidestepping the egress-Host mangling that
    // breaks every direct/cluster-reuse call.
    if !state.cfg.a2d.pin_path_prefix.is_empty() {
        return state.cfg.a2d.service.clone();
    }
    let cluster = cluster_from_props(props).or_else(|| {
        decorator
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    if let Some(cluster) = cluster {
        if let Ok(uri) = state.cfg.a2d.base_url.parse::<Uri>() {
            return Some(Service::new(&cluster, uri));
        }
    }
    if allow_config_fallback {
        return state.cfg.a2d.service.clone();
    }
    None
}

/// Build the outbound A²D client from the typed config.
fn a2d_client(state: &PolicyState) -> A2dClient {
    let reference = A2dTarget {
        base_url: state.cfg.a2d.base_url.clone(),
        asset_id: state.cfg.a2d.asset_id.clone(),
        api_key: state.cfg.a2d.api_key_secret_ref.clone(),
        path_prefix: state.cfg.a2d.pin_path_prefix.clone(),
    };
    A2dClient::new(reference, Duration::from_millis(state.cfg.a2d.pdp_timeout_ms as u64))
}

/// Lazy spec fetch. Runs on the first request/response that has no spec
/// loaded, and again after the configured refresh interval has elapsed.
/// Outbound HTTP from the request/response phases works under managed
/// Flex Gateway; the same call from `configure()` or a background timer
/// never connects. The `service` is resolved to the request's own
/// upstream cluster (or the loopback listener) so the egress `Host`
/// reaches A²D intact. Used by every decision source: `cache` reads the
/// diff straight from it, `remote-pdp`/`hybrid` keep it as the local
/// baseline alongside the PDP audit.
async fn ensure_spec_loaded(state: &PolicyState, client: &HttpClient, service: &Service) {
    let now = now_epoch_secs();
    let should_refresh = {
        let borrow = state.spec.borrow();
        match borrow.as_ref() {
            None => true,
            Some(spec) => {
                let age = now.saturating_sub(spec.fetched_at_epoch_secs);
                age >= state.cfg.a2d.refresh_interval_secs.max(1) as u64
            }
        }
    };
    if !should_refresh {
        return;
    }

    let a2d = a2d_client(state);
    match a2d.fetch_spec(client, service, now).await {
        Ok(spec) => {
            let asset_version = spec.asset_version.clone();
            let tool_count = spec.tools.len();
            let first_load = state.spec.borrow().is_none();
            state.spec.replace(Some(spec));
            logger::info!(
                "mcp-drift-a2d: spec loaded (first_load={} asset_version={} tools={})",
                first_load,
                asset_version,
                tool_count
            );
        }
        Err(e) => {
            logger::warn!("mcp-drift-a2d: spec fetch failed: {e}");
        }
    }
}

/// Best-effort PDP audit for `remote-pdp` / `hybrid` sources. Enforcement
/// is always driven by the local spec diff (so a PDP outage never blocks
/// traffic); this call surfaces the central verdict for correlation. In
/// `hybrid` mode it fires only for the deterministically sampled fraction
/// of requests.
async fn maybe_audit_pdp(
    state: &PolicyState,
    client: &HttpClient,
    service: &Service,
    body: &[u8],
    is_sse: bool,
) {
    match state.cfg.decision.source {
        DecisionSource::RemotePdp | DecisionSource::Hybrid => {}
        DecisionSource::Cache => return,
    }
    let Some(tools) = first_tools_array(body, is_sse) else {
        return;
    };
    if matches!(state.cfg.decision.source, DecisionSource::Hybrid) {
        let correlation = format!("{}:{}", state.cfg.a2d.asset_id, tools.len());
        if !sample_should_fire(state.cfg.decision.hybrid_sample_rate, &correlation) {
            return;
        }
    }
    let a2d = a2d_client(state);
    match a2d.validate(client, service, &tools).await {
        Ok(verdict) => logger::info!(
            "mcp-drift-a2d: pdp verdict kept={} blocked={} ver={}",
            verdict.kept.len(),
            verdict.blocked.len(),
            verdict.asset_version
        ),
        Err(e) => logger::debug!("mcp-drift-a2d: pdp validate failed (best-effort): {e}"),
    }
}

/// Extract the first `tools/list` tools array from a response body,
/// across both the SSE and plain-JSON transports.
fn first_tools_array(body: &[u8], is_sse: bool) -> Option<Vec<serde_json::Value>> {
    if is_sse {
        for ev in sse::parse(body) {
            if let Some(data) = ev.data.as_deref() {
                if let Ok(resp) = serde_json::from_str::<jsonrpc::JsonRpcResponse>(data) {
                    if let Some(tools) = jsonrpc::extract_tools_array(&resp) {
                        return Some(tools.clone());
                    }
                }
            }
        }
        None
    } else {
        let resp: jsonrpc::JsonRpcResponse = serde_json::from_slice(body).ok()?;
        jsonrpc::extract_tools_array(&resp).map(|t| t.clone())
    }
}

/// Request-phase handler. We do not care about the request body; we only
/// run here to warm the spec cache with an outbound call. Outbound HTTP
/// is safe in the request-headers phase (before `into_body_state().await`)
/// and in the response phase; from `configure()` or after the request
/// body-state transition it traps under managed Flex Gateway.
async fn request_filter(
    _request: RequestHeadersState,
    state: PolicyState,
    client: HttpClient,
    props: StreamProperties,
) -> Flow<()> {
    // Only warm the spec here if the upstream cluster is already known.
    // In many runtime modes the cluster isn't selected until after the
    // request headers phase, in which case we defer to the response phase
    // (where `cluster_name` is reliably populated). We do NOT fall back to
    // the config `format: service` cluster here because its egress Host is
    // mangled by the gateway.
    if let Some(service) = resolve_outbound_service(&state, &props, None, false) {
        ensure_spec_loaded(&state, &client, &service).await;
    }
    Flow::Continue(())
}

async fn response_filter(
    headers_state: ResponseHeadersState,
    state: PolicyState,
    client: HttpClient,
    _data: RequestData<()>,
    props: StreamProperties,
) {
    let headers = headers_state.handler().headers();
    let ct = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    let is_sse = ct.contains("text/event-stream");
    let is_json = ct.contains("application/json");
    if !is_sse && !is_json {
        return;
    }

    // Second-chance spec load. The response phase is a safe outbound
    // context AND the phase where the upstream cluster is reliably known,
    // so resolve the outbound service to the request's own upstream
    // cluster (with `x-envoy-decorator-operation` as a fallback source).
    let decorator = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-envoy-decorator-operation"))
        .map(|(_, v)| v.clone());
    let service = resolve_outbound_service(&state, &props, decorator.as_deref(), true);
    if let Some(service) = service.as_ref() {
        ensure_spec_loaded(&state, &client, service).await;
    }

    // Strip content-length on the headers handler BEFORE moving to body
    // state. The rewrite (or no-op re-serialize) may change body length.
    headers_state.handler().remove_header(CONTENT_LENGTH_HEADER);

    let body_state = headers_state.into_body_state().await;
    let body = body_state.handler().body().to_vec();

    // Best-effort central audit for remote-pdp / hybrid sources.
    if let Some(service) = service.as_ref() {
        maybe_audit_pdp(&state, &client, service, &body, is_sse).await;
    }

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
        "mcp-drift-a2d: asset={} base={} source={:?} mode={:?} prefix={:?} service_bound={}",
        cfg.a2d.asset_id,
        cfg.a2d.base_url,
        cfg.decision.source,
        cfg.mode,
        cfg.a2d.pin_path_prefix,
        cfg.a2d.service.is_some(),
    );

    let state = PolicyState {
        cfg: Rc::new(cfg),
        spec: Rc::new(RefCell::new(None)),
        debouncer: Rc::new(RefCell::new(Debouncer::default())),
    };

    if state.cfg.a2d.service.is_none() {
        logger::warn!(
            "mcp-drift-a2d: baseUrl service unbound; response path will emit spec_unavailable evidence and obey failOpen.onSpecUnavailable"
        );
    }

    let request_state = state.clone();
    let response_state = state;
    let filter = on_request(
        move |request: RequestHeadersState, client: HttpClient, props: StreamProperties| {
            let s = request_state.clone();
            async move { request_filter(request, s, client, props).await }
        },
    )
    .on_response(
        move |response: ResponseHeadersState,
              client: HttpClient,
              data: RequestData<()>,
              props: StreamProperties| {
            let s = response_state.clone();
            async move { response_filter(response, s, client, data, props).await }
        },
    );

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
